extern crate elements_miniscript as miniscript;
extern crate regex;

use miniscript::{Descriptor, DummyKey};
use regex::Regex;
use std::str::FromStr;

fn do_test(data: &[u8]) {
    let s = String::from_utf8_lossy(data);
    if let Ok(desc) = Descriptor::<DummyKey>::from_str(&s) {
        let str2 = desc.to_string();
        let desc2 = Descriptor::<DummyKey>::from_str(&str2).unwrap();

        let multi_wrap_pk_re = Regex::new("([a-z]+)c:pk_k\\(").unwrap();
        let multi_wrap_pkh_re = Regex::new("([a-z]+)c:pk_h\\(").unwrap();

        // Before doing anything check the special case
        // To make sure that el are not treated as wrappers
        let normalize_aliases = s
            .replace("elc:pk_h(", "elpkh(")
            .replace("elc:pk_k(", "elpk(");
        let normalize_aliases = multi_wrap_pk_re.replace_all(&normalize_aliases, "$1:pk(");
        let normalize_aliases = multi_wrap_pkh_re.replace_all(&normalize_aliases, "$1:pkh(");
        let normalize_aliases = normalize_aliases
            .replace("c:pk_k(", "pk(")
            .replace("c:pk_h(", "pkh(");

        let mut checksum_split = output.split('#');
        let pre_checksum = checksum_split.next().unwrap();
        assert!(checksum_split.next().is_some());
        assert!(checksum_split.next().is_none());

        if normalize_aliases.len() == output.len() {
            let len = pre_checksum.len();
            assert_eq!(
                normalize_aliases[..len].to_lowercase(),
                pre_checksum.to_lowercase()
            );
        } else {
            assert_eq!(
                normalize_aliases.to_lowercase(),
                pre_checksum.to_lowercase()
            );
        }
    }
}

#[cfg(feature = "afl")]
extern crate afl;
#[cfg(feature = "afl")]
fn main() {
    afl::read_stdio_bytes(|data| {
        do_test(&data);
    });
}

#[cfg(feature = "honggfuzz")]
#[macro_use]
extern crate honggfuzz;
#[cfg(feature = "honggfuzz")]
fn main() {
    loop {
        fuzz!(|data| {
            do_test(data);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test() {
        do_test(b"elc:pk_h()");
    }
}
