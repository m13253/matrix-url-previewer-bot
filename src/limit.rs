#[allow(dead_code)] // Not in use yet
pub fn length_in_bytes(mut s: String, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s;
    }
    for i in (0..=max_bytes.saturating_sub(3)).rev() {
        if s.is_char_boundary(i) {
            s.truncate(i);
            if !s.ends_with("…") {
                s.push_str("…");
            }
            return s;
        }
    }
    unreachable!();
}

pub fn length_in_chars(mut s: String, max_chars: usize) -> String {
    let mut prev_idx_byte = 0;
    for (idx_char, (idx_byte, _)) in s.char_indices().enumerate() {
        if idx_char >= max_chars {
            s.truncate(prev_idx_byte);
            if !s.ends_with("…") {
                s.push_str("…");
            }
            return s;
        }
        prev_idx_byte = idx_byte;
    }
    s
}
