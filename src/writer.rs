use anyhow::Context;
use pkarr::{
    Keypair as PkarrKeypair, SignedPacket,
    dns::rdata::SVCB,
};
use pubky::{Keypair, Pubky, PubkyHttpClient, PublicKey, PubkySession};
use serde_json;

use crate::config::HomeserverConfig;
use crate::link_post::{post_path, LinkPost};

pub struct PubkyWriter {
    mode: WriterMode,
    pubkey_str: String,
}

enum WriterMode {
    Pubky(PubkySession),
    Direct {
        client: reqwest::Client,
        base_url: String,
        cookie: String,
    },
}

impl PubkyWriter {
    pub async fn connect(config: &HomeserverConfig) -> anyhow::Result<Self> {
        let secret_bytes =
            hex::decode(&config.secret_key).context("secret_key must be valid hex")?;

        let secret: [u8; 32] = secret_bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("secret key must be exactly 32 bytes"))?;
        let keypair = Keypair::from_secret(&secret);

        let raw_pubkey_str = keypair.public_key().to_string();
        let pubkey_str = raw_pubkey_str.strip_prefix("pubky").unwrap_or(&raw_pubkey_str).to_string();
        tracing::info!(pubkey = %pubkey_str, "connecting to homeserver");

        if let Some(direct_url) = &config.direct_url {
            return Self::connect_direct(config, &keypair, &pubkey_str, direct_url).await;
        }

        let homeserver = PublicKey::try_from(config.pubkey.clone())
            .map_err(|e| anyhow::anyhow!("invalid homeserver pubkey: {}", e))?;

        let pubky_client = if config.testnet {
            let host = config.testnet_host.as_deref().unwrap_or("localhost");
            tracing::info!(testnet_host = %host, "using testnet mode");
            let client = PubkyHttpClient::builder()
                .testnet_with_host(host)
                .build()?;
            Pubky::with_client(client)
        } else {
            Pubky::new()?
        };
        let signer = pubky_client.signer(keypair);

        let session = match signer.signin().await {
            Ok(session) => {
                tracing::info!("signed in via pubky SDK");
                session
            }
            Err(e) => {
                tracing::info!(error = %e, "signin failed, attempting signup via pubky SDK");
                signer
                    .signup(&homeserver, config.signup_code.as_deref())
                    .await
                    .map_err(|e| anyhow::anyhow!("failed to signup: {}", e))?
            }
        };

        Ok(Self {
            mode: WriterMode::Pubky(session),
            pubkey_str,
        })
    }

    async fn connect_direct(
        config: &HomeserverConfig,
        keypair: &Keypair,
        pubkey_str: &str,
        direct_url: &str,
    ) -> anyhow::Result<Self> {
        tracing::info!(url = %direct_url, "connecting directly to homeserver (bypassing pkarr)");

        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .build()?;

        let base = direct_url.trim_end_matches('/');
        let signup_url = format!("{}/signup", base);
        let session_url = format!("{}/session", base);

        let make_token = || {
            let caps = pubky::Capabilities::builder()
                .cap(pubky::Capability::root())
                .finish();
            pubky::AuthToken::sign(keypair, caps).serialize()
        };

        let resp = client
            .post(&session_url)
            .header("pubky-host", pubkey_str)
            .body(make_token())
            .send()
            .await;

        let resp = match resp {
            Ok(r) if r.status().is_success() => {
                tracing::info!("signed in directly");
                r
            }
            _ => {
                tracing::info!("direct signin failed, trying signup");
                let signup_resp = client
                    .post(&signup_url)
                    .header("pubky-host", pubkey_str)
                    .body(make_token())
                    .send()
                    .await?;

                if !signup_resp.status().is_success() {
                    let status = signup_resp.status();
                    let body = signup_resp.text().await.unwrap_or_default();
                    anyhow::bail!("signup failed: HTTP {} - {}", status, body);
                }
                tracing::info!("signed up directly");
                signup_resp
            }
        };

        let cookie = resp
            .headers()
            .get_all("set-cookie")
            .iter()
            .find_map(|v| {
                let s = v.to_str().ok()?;
                Some(s.split(';').next()?.to_string())
            })
            .ok_or_else(|| anyhow::anyhow!("no session cookie in response"))?;

        tracing::info!("obtained session cookie for direct mode");

        if let Some(relay_url) = &config.pkarr_relay_url {
            if let Err(e) = Self::publish_pkarr_record(
                &client,
                &config.secret_key,
                &config.pubkey,
                relay_url,
            )
            .await
            {
                tracing::warn!(error = %e, "failed to publish pkarr record (nexus may not be able to resolve this bot)");
            }
        }

        Ok(Self {
            mode: WriterMode::Direct {
                client,
                base_url: direct_url.trim_end_matches('/').to_string(),
                cookie,
            },
            pubkey_str: pubkey_str.to_string(),
        })
    }

    async fn publish_pkarr_record(
        client: &reqwest::Client,
        bot_secret_hex: &str,
        homeserver_pubkey: &str,
        relay_url: &str,
    ) -> anyhow::Result<()> {
        let secret_bytes = hex::decode(bot_secret_hex)?;
        let secret: [u8; 32] = secret_bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("secret key must be 32 bytes"))?;
        let pkarr_kp = PkarrKeypair::from_secret_key(&secret);

        let svcb = SVCB::new(0, homeserver_pubkey.try_into()
            .map_err(|e| anyhow::anyhow!("invalid homeserver pubkey for SVCB: {:?}", e))?);
        let pubky_name = "_pubky".try_into()
            .map_err(|e| anyhow::anyhow!("invalid _pubky name: {:?}", e))?;

        let packet = SignedPacket::builder()
            .https(pubky_name, svcb, 3600)
            .sign(&pkarr_kp)
            .map_err(|e| anyhow::anyhow!("failed to sign pkarr packet: {:?}", e))?;

        let z32 = pkarr_kp.to_z32();
        let url = format!("{}/{}", relay_url.trim_end_matches('/'), z32);
        let payload = packet.to_relay_payload();

        tracing::info!(url = %url, z32 = %z32, "publishing pkarr record to relay");

        let resp = client
            .put(&url)
            .body(payload.to_vec())
            .send()
            .await?;

        if resp.status().is_success() {
            tracing::info!("pkarr record published successfully");
        } else {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("pkarr relay returned HTTP {} - {}", status, body);
        }

        Ok(())
    }

    pub fn pubkey(&self) -> &str {
        &self.pubkey_str
    }

    pub async fn publish_profile(&self) -> anyhow::Result<()> {
        let profile_json = serde_json::json!({
            "name": "Pubky Web Index",
            "bio": "Automated bot that indexes web links for discovery in Pubky App.",
        });
        let body = serde_json::to_vec(&profile_json)?;
        let path = "/pub/pubky.app/profile.json";

        match &self.mode {
            WriterMode::Pubky(session) => {
                let full_path = format!("pubky://{}{}", self.pubkey_str, path);
                session
                    .storage()
                    .put(&full_path, body)
                    .await
                    .map_err(|e| anyhow::anyhow!("PUT profile failed: {}", e))?;
            }
            WriterMode::Direct {
                client,
                base_url,
                cookie,
            } => {
                let url = format!("{}{}", base_url, path);
                let resp = client
                    .put(&url)
                    .header("cookie", cookie.as_str())
                    .header("pubky-host", self.pubkey_str.as_str())
                    .body(body)
                    .send()
                    .await?;

                if !resp.status().is_success() {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    anyhow::bail!("PUT profile failed: HTTP {} - {}", status, text);
                }
            }
        }

        tracing::info!("published bot user profile");
        Ok(())
    }

    pub async fn publish_link_post(&self, post_id: &str, post: &LinkPost) -> anyhow::Result<()> {
        let path = post_path(post_id);
        let body = post.to_json()?;

        match &self.mode {
            WriterMode::Pubky(session) => {
                let full_path = format!("pubky://{}{}", self.pubkey_str, path);
                session
                    .storage()
                    .put(&full_path, body)
                    .await
                    .map_err(|e| anyhow::anyhow!("PUT failed: {}", e))?;
            }
            WriterMode::Direct {
                client,
                base_url,
                cookie,
            } => {
                let url = format!("{}{}", base_url, path);
                let resp = client
                    .put(&url)
                    .header("cookie", cookie.as_str())
                    .header("pubky-host", self.pubkey_str.as_str())
                    .body(body)
                    .send()
                    .await?;

                if !resp.status().is_success() {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    anyhow::bail!("PUT failed: HTTP {} - {}", status, body);
                }
            }
        }

        tracing::debug!(post_id = %post_id, "published link post");
        Ok(())
    }

    pub async fn publish_tag(&self, tag: &crate::link_post::PubkyTag) -> anyhow::Result<()> {
        let path = tag.path();
        let body = tag.to_json()?;

        match &self.mode {
            WriterMode::Pubky(session) => {
                let full_path = format!("pubky://{}{}", self.pubkey_str, path);
                session
                    .storage()
                    .put(&full_path, body)
                    .await
                    .map_err(|e| anyhow::anyhow!("PUT tag failed: {}", e))?;
            }
            WriterMode::Direct {
                client,
                base_url,
                cookie,
            } => {
                let url = format!("{}{}", base_url, path);
                let resp = client
                    .put(&url)
                    .header("cookie", cookie.as_str())
                    .header("pubky-host", self.pubkey_str.as_str())
                    .body(body)
                    .send()
                    .await?;

                if !resp.status().is_success() {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    anyhow::bail!("PUT tag failed: HTTP {} - {}", status, body);
                }
            }
        }

        tracing::debug!(label = %tag.label, "published tag");
        Ok(())
    }
}
