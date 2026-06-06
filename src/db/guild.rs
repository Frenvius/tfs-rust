use crate::db::{Database, DatabaseEngine};
use crate::world::guild::{Guild, GuildWarVector};

pub async fn load_guild(guild_id: u32, db: &Database) -> Option<Guild> {
    let result = db
        .store_query(&format!(
            "SELECT `name` FROM `guilds` WHERE `id` = {guild_id}"
        ))
        .await
        .ok()??;

    let name = result.get_string("name")?;
    let mut guild = Guild::new(guild_id, name);

    if let Ok(Some(mut ranks)) = db
        .store_query(&format!(
            "SELECT `id`, `name`, `level` FROM `guild_ranks` WHERE `guild_id` = {guild_id}"
        ))
        .await
    {
        loop {
            let rank_id = ranks.get_u64("id")? as u32;
            let rank_name = ranks.get_string("name")?;
            let level = ranks.get_u64("level")? as u8;
            guild.add_rank(rank_id, rank_name, level);
            if !ranks.next() {
                break;
            }
        }
    }

    Some(guild)
}

pub async fn get_guild_id_by_name(name: &str, db: &Database) -> u32 {
    let escaped = db.escape_string(name);
    let result = db
        .store_query(&format!(
            "SELECT `id` FROM `guilds` WHERE `name` = {escaped}"
        ))
        .await;

    match result {
        Ok(Some(row)) => row.get_u64("id").unwrap_or(0) as u32,
        _ => 0,
    }
}

pub async fn get_war_list(guild_id: u32, db: &Database) -> GuildWarVector {
    let mut out = GuildWarVector::new();

    let result = db
        .store_query(&format!(
            "SELECT `guild1`, `guild2` FROM `guild_wars` \
             WHERE (`guild1` = {guild_id} OR `guild2` = {guild_id}) \
             AND `ended` = 0 AND `status` = 1"
        ))
        .await;

    let mut rows = match result {
        Ok(Some(r)) => r,
        _ => return out,
    };

    loop {
        let guild1 = rows.get_u64("guild1").unwrap_or(0) as u32;
        let guild2 = rows.get_u64("guild2").unwrap_or(0) as u32;
        if guild_id != guild1 {
            out.push(guild1);
        } else {
            out.push(guild2);
        }
        if !rows.next() {
            break;
        }
    }

    out
}
