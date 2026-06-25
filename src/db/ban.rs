use crate::db::{g_database, DatabaseEngine};

pub struct BanInfo {
    pub reason: String,
    pub expires_at: i64,
}

pub async fn get_account_ban_info(account_id: u32) -> Option<BanInfo> {
    let db = g_database();
    let query = format!(
        "SELECT `reason`, `expires_at`, `banned_at`, `banned_by`, \
         (SELECT `name` FROM `players` WHERE `id` = `banned_by`) AS `name` \
         FROM `account_bans` WHERE `account_id` = {}",
        account_id
    );
    let result = db.store_query(&query).await.ok()??;

    let expires_at = result.get_i64("expires_at").unwrap_or(0);
    if expires_at != 0 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        if now > expires_at {
            let reason_esc = db.escape_string(&result.get_string("reason").unwrap_or_default());
            let banned_at = result.get_i64("banned_at").unwrap_or(0);
            let banned_by_id = result.get_u64("banned_by").unwrap_or(0);
            let _ = db.execute(&format!(
                "INSERT INTO `account_ban_history` (`account_id`, `reason`, `banned_at`, `expired_at`, `banned_by`) \
                 VALUES ({}, {}, {}, {}, {})",
                account_id, reason_esc, banned_at, expires_at, banned_by_id
            )).await;
            let _ = db.execute(&format!(
                "DELETE FROM `account_bans` WHERE `account_id` = {}",
                account_id
            )).await;
            return None;
        }
    }

    let reason = result.get_string("reason").unwrap_or_default();
    Some(BanInfo {
        reason: if reason.is_empty() { "(none)".to_string() } else { reason },
        expires_at,
    })
}

pub async fn get_ip_ban_info(ip: &str) -> Option<BanInfo> {
    if ip.is_empty() || ip == "0.0.0.0" || ip == "::" {
        return None;
    }
    let db = g_database();
    let ip_esc = db.escape_string(ip);
    let query = format!(
        "SELECT `reason`, `expires_at`, (SELECT `name` FROM `players` WHERE `id` = `banned_by`) AS `name` \
         FROM `ip_bans` WHERE `ip` = INET6_ATON({})",
        ip_esc
    );
    let result = db.store_query(&query).await.ok()??;

    let expires_at = result.get_i64("expires_at").unwrap_or(0);
    if expires_at != 0 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        if now > expires_at {
            let _ = db.execute(&format!(
                "DELETE FROM `ip_bans` WHERE `ip` = INET6_ATON({})", ip_esc
            )).await;
            return None;
        }
    }

    let reason = result.get_string("reason").unwrap_or_default();
    Some(BanInfo {
        reason: if reason.is_empty() { "(none)".to_string() } else { reason },
        expires_at,
    })
}
