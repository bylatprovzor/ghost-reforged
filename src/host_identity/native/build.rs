//! Exact executable/SDK pairs inspected for the host-profile send filter.
pub(super) struct Build {
    executable: &'static str,
    sdk: &'static str,
    pub table: usize,
    pub methods: [usize; 9],
}

const BUILDS: [Build; 3] = [
    Build {
        // Warcraft III 3.0.0.24268.
        executable: "bd2a0dc256289de45287bb60f3725b88f1235216d5177984377fe1ca22840a12",
        sdk: "3a8762f6641f39da8099009adccfe1b2f9aa9613defe98273e38341de500b10e",
        table: 0x7777b8,
        methods: [
            0xaa630, 0xabdc0, 0xaaa10, 0xaadb0, 0xabc70, 0xab220, 0xabc40, 0xab040, 0xaaad0,
        ],
    },
    Build {
        // Warcraft III 3.0.1.24370; the SDK table remains at the same RVA.
        executable: "dd9df98e779421fa02387dde7eed3edce09c69d8bee4dbfa25d9bdebeb01bb75",
        sdk: "04f798ac211b9fea1b741b4f1d520d7e5b3cf5f038241c909b7429b02a4cf134",
        table: 0x7777b8,
        methods: [
            0xaa730, 0xabec0, 0xaab10, 0xaaeb0, 0xabd70, 0xab320, 0xabd40, 0xab140, 0xaabd0,
        ],
    },
    Build {
        // Warcraft III 3.0.1.24371. ClientSdk.dll is byte-identical to 24370.
        executable: "eb39ab298ae1d84d307aba4d400f2a7dbda2900a46964bb28aeba592f45fa97a",
        sdk: "04f798ac211b9fea1b741b4f1d520d7e5b3cf5f038241c909b7429b02a4cf134",
        table: 0x7777b8,
        methods: [
            0xaa730, 0xabec0, 0xaab10, 0xaaeb0, 0xabd70, 0xab320, 0xabd40, 0xab140, 0xaabd0,
        ],
    },
];

pub(super) fn for_hashes(executable: &str, sdk: &str) -> Option<&'static Build> {
    BUILDS
        .iter()
        .find(|build| build.executable == executable && build.sdk == sdk)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_pairs_select_their_own_send_address() {
        let old = &BUILDS[0];
        let new = &BUILDS[1];
        assert_eq!(
            for_hashes(old.executable, old.sdk).unwrap().methods[7],
            0xab040
        );
        assert_eq!(
            for_hashes(new.executable, new.sdk).unwrap().methods[7],
            0xab140
        );
        assert!(for_hashes(old.executable, new.sdk).is_none());
        assert!(for_hashes(new.executable, old.sdk).is_none());
        assert!(for_hashes("unknown executable", new.sdk).is_none());
        assert!(for_hashes(new.executable, "unknown SDK").is_none());
    }

    #[test]
    fn hotfix_24371_requires_the_exact_unchanged_sdk() {
        let hotfix = for_hashes(
            "eb39ab298ae1d84d307aba4d400f2a7dbda2900a46964bb28aeba592f45fa97a",
            "04f798ac211b9fea1b741b4f1d520d7e5b3cf5f038241c909b7429b02a4cf134",
        ).expect("verified 24371 pair");
        assert_eq!(hotfix.table, BUILDS[1].table);
        assert_eq!(hotfix.methods, BUILDS[1].methods);
        assert!(for_hashes(hotfix.executable, BUILDS[0].sdk).is_none());
        assert!(for_hashes(hotfix.executable, "unknown SDK").is_none());
        assert_eq!(crate::config::Config::default().expected_version, "3.0.1.24371");
    }
}
