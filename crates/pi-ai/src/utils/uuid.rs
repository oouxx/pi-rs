//! Time-ordered UUIDv7 generation (match TS `packages/ai/src/utils/uuid.ts`).
//!
//! The TS implementation keeps a monotonic sequence counter so ids generated
//! within the same millisecond stay ordered. We mirror that with a global
//! `Mutex<UuidV7State>`.

use std::sync::Mutex;

struct UuidV7State {
    last_timestamp: i64,
    sequence: u32,
}

static STATE: Mutex<UuidV7State> = Mutex::new(UuidV7State {
    last_timestamp: -1,
    sequence: 0,
});

fn fill_random_bytes(bytes: &mut [u8]) {
    // Prefer OS randomness; fall back to a simple PRNG seeded from the clock.
    use rand::RngCore;
    let mut rng = rand::rngs::OsRng;
    rng.fill_bytes(bytes);
}

/// Generate a time-ordered UUIDv7 string (match TS `uuidv7`).
pub fn uuid_v7() -> String {
    let mut random = [0u8; 16];
    fill_random_bytes(&mut random);

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    let mut state = STATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let sequence = if timestamp > state.last_timestamp {
        state.last_timestamp = timestamp;
        (u32::from(random[6]) << 24)
            | (u32::from(random[7]) << 16)
            | (u32::from(random[8]) << 8)
            | u32::from(random[9])
    } else {
        let next = state.sequence.wrapping_add(1);
        state.sequence = next;
        if next == 0 {
            state.last_timestamp += 1;
        }
        next
    };

    let mut bytes = [0u8; 16];
    let ts = state.last_timestamp as u64;
    bytes[0] = (ts >> 40) as u8;
    bytes[1] = (ts >> 32) as u8;
    bytes[2] = (ts >> 24) as u8;
    bytes[3] = (ts >> 16) as u8;
    bytes[4] = (ts >> 8) as u8;
    bytes[5] = ts as u8;
    bytes[6] = 0x70 | ((sequence >> 28) as u8 & 0x0f);
    bytes[7] = (sequence >> 20) as u8;
    bytes[8] = 0x80 | ((sequence >> 14) as u8 & 0x3f);
    bytes[9] = (sequence >> 6) as u8;
    bytes[10] = (((sequence & 0x3f) << 2) as u8) | (random[10] & 0x03);
    bytes[11] = random[11];
    bytes[12] = random[12];
    bytes[13] = random[13];
    bytes[14] = random[14];
    bytes[15] = random[15];

    let hex: Vec<String> = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        hex[0..4].concat(),
        hex[4..6].concat(),
        hex[6..8].concat(),
        hex[8..10].concat(),
        hex[10..16].concat()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uuid_v7_format() {
        let id = uuid_v7();
        // 8-4-4-4-12 hex groups
        let parts: Vec<&str> = id.split('-').collect();
        assert_eq!(parts.len(), 5);
        assert_eq!(parts[0].len(), 8);
        assert_eq!(parts[1].len(), 4);
        assert_eq!(parts[2].len(), 4);
        assert_eq!(parts[3].len(), 4);
        assert_eq!(parts[4].len(), 12);
        // version nibble = 7
        assert_eq!(&parts[2][..1], "7");
        // variant bits = 10xx
        assert!(parts[3].starts_with('8') || parts[3].starts_with('9') || parts[3].starts_with('a') || parts[3].starts_with('b'));
    }

    #[test]
    fn test_uuid_v7_unique() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..1000 {
            let id = uuid_v7();
            assert!(seen.insert(id.clone()), "duplicate uuidv7: {id}");
        }
    }
}
