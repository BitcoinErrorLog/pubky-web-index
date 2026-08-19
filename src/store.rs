use rusqlite::{params, Connection};
use serde::Serialize;
use std::path::Path;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct UrlStore {
    conn: Arc<Mutex<Connection>>,
}

#[derive(Debug, Serialize, Clone)]
pub struct IndexedUrl {
    pub url: String,
    pub post_id: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub image_url: Option<String>,
    pub site_name: Option<String>,
    pub domain: Option<String>,
    pub language: Option<String>,
    pub source: String,
    pub published_at: String,
}

impl UrlStore {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(path)?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS urls (
                url TEXT PRIMARY KEY,
                post_id TEXT NOT NULL,
                title TEXT,
                description TEXT,
                image_url TEXT,
                site_name TEXT,
                domain TEXT,
                language TEXT,
                published_at TEXT NOT NULL DEFAULT (datetime('now')),
                source TEXT NOT NULL DEFAULT 'direct'
            );
            CREATE INDEX IF NOT EXISTS idx_urls_source ON urls(source);
            CREATE INDEX IF NOT EXISTS idx_urls_published ON urls(published_at);
            CREATE INDEX IF NOT EXISTS idx_urls_language ON urls(language);
            CREATE INDEX IF NOT EXISTS idx_urls_domain ON urls(domain);",
        )?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn has_url(&self, url: &str) -> anyhow::Result<bool> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM urls WHERE url = ?1",
            params![url],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    pub fn insert_url(
        &self,
        url: &str,
        post_id: &str,
        title: Option<&str>,
        description: Option<&str>,
        image_url: Option<&str>,
        site_name: Option<&str>,
        domain: Option<&str>,
        language: Option<&str>,
        source: &str,
    ) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO urls (url, post_id, title, description, image_url, site_name, domain, language, source)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![url, post_id, title, description, image_url, site_name, domain, language, source],
        )?;
        Ok(())
    }

    pub fn search(&self, query: &str, limit: usize, offset: usize) -> anyhow::Result<Vec<IndexedUrl>> {
        let conn = self.conn.lock().unwrap();
        let pattern = format!("%{}%", query);
        let mut stmt = conn.prepare(
            "SELECT url, post_id, title, description, image_url, site_name, domain, language, source, published_at
             FROM urls
             WHERE title LIKE ?1 OR description LIKE ?1 OR url LIKE ?1 OR domain LIKE ?1
             ORDER BY published_at DESC
             LIMIT ?2 OFFSET ?3"
        )?;

        let rows = stmt.query_map(params![pattern, limit as i64, offset as i64], |row| {
            Ok(IndexedUrl {
                url: row.get(0)?,
                post_id: row.get(1)?,
                title: row.get(2)?,
                description: row.get(3)?,
                image_url: row.get(4)?,
                site_name: row.get(5)?,
                domain: row.get(6)?,
                language: row.get(7)?,
                source: row.get(8)?,
                published_at: row.get(9)?,
            })
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    pub fn recent(&self, limit: usize) -> anyhow::Result<Vec<IndexedUrl>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT url, post_id, title, description, image_url, site_name, domain, language, source, published_at
             FROM urls
             ORDER BY published_at DESC
             LIMIT ?1"
        )?;

        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok(IndexedUrl {
                url: row.get(0)?,
                post_id: row.get(1)?,
                title: row.get(2)?,
                description: row.get(3)?,
                image_url: row.get(4)?,
                site_name: row.get(5)?,
                domain: row.get(6)?,
                language: row.get(7)?,
                source: row.get(8)?,
                published_at: row.get(9)?,
            })
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    pub fn count(&self) -> anyhow::Result<i64> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM urls", [], |row| row.get(0))?;
        Ok(count)
    }

    pub fn count_by_source(&self, source: &str) -> anyhow::Result<i64> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM urls WHERE source = ?1",
            params![source],
            |row| row.get(0),
        )?;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_store_operations() {
        let path = PathBuf::from("/tmp/pubky-web-index-test.db");
        let _ = std::fs::remove_file(&path);

        let store = UrlStore::open(&path).unwrap();

        assert!(!store.has_url("https://example.com").unwrap());

        store
            .insert_url(
                "https://example.com", "ABC123", Some("Example"), Some("A description"),
                Some("https://example.com/img.jpg"), Some("Example Site"), Some("example.com"),
                Some("en"), "direct",
            )
            .unwrap();

        assert!(store.has_url("https://example.com").unwrap());
        assert_eq!(store.count().unwrap(), 1);
        assert_eq!(store.count_by_source("direct").unwrap(), 1);

        let results = store.search("Example", 10, 0).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title.as_deref(), Some("Example"));

        let recent = store.recent(10).unwrap();
        assert_eq!(recent.len(), 1);

        let _ = std::fs::remove_file(&path);
    }
}
