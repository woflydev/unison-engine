//! What a reaction chip says on hover: who reacted, in Discord's wording.

/// "A reacted with :x:", "A and B reacted with :x:", "A, B and C reacted ...",
/// "A, B, C and 4 others reacted with :x:". `total` is the chip's count (more than `names` when
/// the list was cut short).
pub fn summary(names: &[String], total: u32, emoji: &str) -> String {
    let shown: Vec<&String> = names.iter().take(3).collect();
    let others = (total as usize).saturating_sub(shown.len());
    let who = match (shown.len(), others) {
        (0, _) => return format!("{total} {} with {emoji}", if total == 1 { "reaction" } else { "reactions" }),
        (1, 0) => shown[0].to_string(),
        (n, 0) => format!("{} and {}", shown[..n - 1].iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "), shown[n - 1]),
        (_, o) => format!("{} and {o} {}", shown.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "), if o == 1 { "other" } else { "others" }),
    };
    format!("{who} reacted with {emoji}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn wording() {
        assert_eq!(summary(&n(&["A"]), 1, ":x:"), "A reacted with :x:");
        assert_eq!(summary(&n(&["A", "B"]), 2, ":x:"), "A and B reacted with :x:");
        assert_eq!(summary(&n(&["A", "B", "C"]), 3, ":x:"), "A, B and C reacted with :x:");
        assert_eq!(summary(&n(&["A", "B", "C", "D"]), 9, ":x:"), "A, B, C and 6 others reacted with :x:");
        assert_eq!(summary(&n(&["A", "B", "C", "D"]), 4, ":x:"), "A, B, C and 1 other reacted with :x:");
        assert_eq!(summary(&[], 5, ":x:"), "5 reactions with :x:");
    }
}
