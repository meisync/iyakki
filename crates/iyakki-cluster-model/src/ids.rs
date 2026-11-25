//! Name rules the registry needs (moved from `k3sdev` and `vault`, which call these).

/// A managed cluster id: 2 to 16 of lowercase letters, digits and `-`, not `local`, no `-` at either end.
pub fn valid_k3s_id(id: &str) -> bool {
    (2..=16).contains(&id.len()) && id != "local" && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') && !id.starts_with('-') && !id.ends_with('-')
}

/// A vault entry name: lowercase letters, digits, `.` `_` `-`, up to 64, starting with a letter or digit.
pub fn valid_secret_name(n: &str) -> bool {
    let mut c = n.chars();
    c.next().is_some_and(|f| f.is_ascii_lowercase() || f.is_ascii_digit()) && n.len() <= 64 && c.all(|x| x.is_ascii_lowercase() || x.is_ascii_digit() || matches!(x, '.' | '_' | '-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cluster_ids_follow_the_rule() {
        assert!(valid_k3s_id("gpu-box") && valid_k3s_id("t2"));
        assert!(!valid_k3s_id("local") && !valid_k3s_id("a") && !valid_k3s_id("-x") && !valid_k3s_id("x-") && !valid_k3s_id("Up"));
        assert!(!valid_k3s_id(&"a".repeat(17)));
    }

    #[test]
    fn secret_names_follow_the_rule() {
        assert!(valid_secret_name("kube.prod_1-a") && valid_secret_name("0abc"));
        assert!(!valid_secret_name("") && !valid_secret_name("_x") && !valid_secret_name("Up") && !valid_secret_name(&"a".repeat(65)));
    }
}
