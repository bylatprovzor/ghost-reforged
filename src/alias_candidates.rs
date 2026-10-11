use std::collections::BTreeSet;

/// Addresses are hints, never authority to write. Memory validates each object again.
#[derive(Default)]
pub(super) struct Candidates {
    pub verified: Option<usize>,
    previous: BTreeSet<usize>,
}

impl Candidates {
    pub fn rank(&self, addresses: &mut [usize]) {
        addresses.sort_unstable_by_key(|address| {
            (
                Some(*address) != self.verified,
                self.previous.contains(address),
                self.verified.map_or(0, |known| known.abs_diff(*address)),
                *address,
            )
        });
    }

    pub fn remember_scan(&mut self, addresses: &[usize]) {
        self.previous = addresses.iter().copied().collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaced_objects_are_tried_before_unchanged_name_copies() {
        let mut hints = Candidates::default();
        hints.remember_scan(&[100, 200, 300]);
        hints.verified = Some(200);
        let mut addresses = vec![100, 190, 200, 300, 600];
        hints.rank(&mut addresses);
        assert_eq!(addresses, [200, 190, 600, 100, 300]);
    }
}
