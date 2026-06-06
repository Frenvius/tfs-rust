use std::time::{SystemTime, UNIX_EPOCH};

use rand::{thread_rng, Rng};

pub mod json5;
pub mod wildcard;

pub fn adler_checksum(data: &[u8]) -> u32 {
    if data.len() > crate::net::message::NETWORK_MESSAGE_MAXSIZE {
        return 0;
    }

    const ADLER: u32 = 65_521;

    let mut a = 1u32;
    let mut b = 0u32;
    let mut remaining = data;

    while !remaining.is_empty() {
        let chunk_len = remaining.len().min(5_552);
        let (chunk, rest) = remaining.split_at(chunk_len);

        for byte in chunk {
            a += u32::from(*byte);
            b += a;
        }

        a %= ADLER;
        b %= ADLER;
        remaining = rest;
    }

    (b << 16) | a
}

/// OTSYS_TIME() equivalent — milliseconds since epoch.
pub fn otsys_time() -> i64 {
    get_milliseconds_time()
}

pub fn get_milliseconds_time() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

pub fn uniform_random(min_val: i64, max_val: i64) -> i64 {
    if min_val >= max_val {
        return min_val;
    }
    thread_rng().gen_range(min_val..=max_val)
}

pub fn normal_random(min_val: i32, max_val: i32) -> i32 {
    if min_val == max_val {
        return min_val;
    }
    let (min_val, max_val) = if min_val > max_val { (max_val, min_val) } else { (min_val, max_val) };
    let diff = max_val - min_val;
    // Box-Muller transform: Normal(0.5, 0.25), clamped to [0, 1]
    let u1: f64 = thread_rng().gen::<f64>().max(f64::EPSILON);
    let u2: f64 = thread_rng().gen::<f64>();
    let z = (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos();
    let v = z * 0.25 + 0.5;
    let increment = if v < 0.0 {
        diff / 2
    } else if v > 1.0 {
        (diff + 1) / 2
    } else {
        (v * diff as f64).round() as i32
    };
    min_val + increment
}

pub fn random_range(min_number: i32, max_number: i32) -> i32 {
    if min_number == max_number {
        return min_number;
    }

    let (min_number, max_number) = if min_number > max_number {
        (max_number, min_number)
    } else {
        (min_number, max_number)
    };

    thread_rng().gen_range(min_number..=max_number)
}

/// clientToServerFluidMap from C++ const.h
const CLIENT_TO_SERVER_FLUID: [u8; 18] = [
    0,  // FLUID_EMPTY
    1,  // FLUID_WATER
    7,  // FLUID_MANA
    3,  // FLUID_BEER
    27, // FLUID_MUD
    5,  // FLUID_BLOOD
    6,  // FLUID_SLIME
    27, // FLUID_RUM
    8,  // FLUID_LEMONADE
    9,  // FLUID_MILK
    14, // FLUID_WINE
    13, // FLUID_LIFE
    12, // FLUID_URINE
    10, // FLUID_OIL
    11, // FLUID_FRUITJUICE
    15, // FLUID_COCONUTMILK
    16, // FLUID_TEA
    17, // FLUID_MEAD
];

pub fn server_fluid_to_client(server_fluid: u8) -> u8 {
    for (i, &sf) in CLIENT_TO_SERVER_FLUID.iter().enumerate() {
        if sf == server_fluid {
            return i as u8;
        }
    }
    0
}

pub fn client_fluid_to_server(client_fluid: u8) -> u8 {
    if client_fluid as usize >= CLIENT_TO_SERVER_FLUID.len() {
        return 0;
    }
    CLIENT_TO_SERVER_FLUID[client_fluid as usize]
}

#[cfg(test)]
mod tests {
    use super::{adler_checksum, get_milliseconds_time, random_range};

    #[test]
    fn adler_checksum_should_match_the_upstream_value() {
        assert_eq!(adler_checksum(b"abc"), 38_600_999);
    }

    #[test]
    fn get_milliseconds_time_should_return_a_positive_epoch_value() {
        assert!(get_milliseconds_time() > 0);
    }

    #[test]
    fn random_range_should_swap_bounds_when_they_are_reversed() {
        let value = random_range(5, 1);

        assert!((1..=5).contains(&value));
    }
}
