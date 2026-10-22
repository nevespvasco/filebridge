//! App settings as key → JSON text (`app`: the bandwidth limit and send window, docs/adr/0010).

use rusqlite::{OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};

use crate::{Result, Store};

impl Store {
    pub async fn get_setting(&self, key: &str) -> Result<Option<String>> {
        let key = key.to_owned();
        self.call(move |c| {
            Ok(
                c.query_row("SELECT value FROM settings WHERE key = ?1", [&key], |r| {
                    r.get(0)
                })
                .optional()?,
            )
        })
        .await
    }

    pub async fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        let (key, value) = (key.to_owned(), value.to_owned());
        self.call(move |c| {
            c.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = ?2",
                params![key, value],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn get_setting_json<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        match self.get_setting(key).await? {
            Some(v) => Ok(Some(serde_json::from_str(&v)?)),
            None => Ok(None),
        }
    }

    pub async fn set_setting_json<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        self.set_setting(key, &serde_json::to_string(value)?).await
    }
}

#[cfg(test)]
mod tests {
    use crate::Store;

    #[tokio::test]
    async fn json_round_trip() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(
            store.get_setting_json::<u32>("bwlimit").await.unwrap(),
            None
        );
        store.set_setting_json("bwlimit", &20_000u32).await.unwrap();
        store.set_setting_json("bwlimit", &30_000u32).await.unwrap();
        assert_eq!(
            store.get_setting_json::<u32>("bwlimit").await.unwrap(),
            Some(30_000)
        );
    }
}
