//! Merging two vaults, so a phone and a PC can be brought back in step.
//!
//! How it works
//! ------------
//! * Every entry has a unique id (`uid`) that is the same on every device, and
//!   a `updated` time. The local number `id` is only for this device.
//! * Merging an entry that exists on both sides keeps the one that was changed
//!   last. If the times are exactly equal, the contents are compared, so both
//!   devices pick the same winner.
//! * Deleting an entry leaves a small record (a "tombstone"), so the deletion
//!   travels to the other device instead of the entry coming back.
//! * Merging only ever changes the vault you merge INTO. Do it on both devices
//!   (each merging the other's file) and they end up identical.
//!
//! The two vaults may have different master passwords, because the other one is
//! opened separately and only its entries are read.

use crate::{Entry, Tombstone, Vault};
use std::collections::HashMap;

/// What a merge did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MergeReport {
    /// Entries that were new to this vault.
    pub added: u32,
    /// Entries replaced by a newer version from the other vault.
    pub updated: u32,
    /// Entries removed because they were deleted on the other vault.
    pub deleted: u32,
    /// Entries that were already the same (or already newer here).
    pub unchanged: u32,
}

/// Everything about an entry except its numbering, in a comparable form.
fn content_key(e: &Entry) -> (String, String, String, String, String, String) {
    (
        e.password.clone(),
        e.site.clone(),
        e.username.clone(),
        e.notes.clone(),
        e.category.clone(),
        e.totp.as_ref().map(|t| t.to_input()).unwrap_or_default(),
    )
}

impl Vault {
    /// Merge the entries of `other` into this vault. Call `save` afterwards.
    pub fn merge_from(&mut self, other: &Vault) -> MergeReport {
        let mut report = MergeReport::default();

        // 1. Deletion records from both sides (the newest for each entry).
        let mut tombs: HashMap<String, u64> = HashMap::new();
        for t in self.data.deleted.iter().chain(other.data.deleted.iter()) {
            let at = tombs.entry(t.uid.clone()).or_insert(0);
            if t.at > *at {
                *at = t.at;
            }
        }

        // 2. Bring in the other vault's entries.
        for o in &other.data.entries {
            // The same entry (same unique id)?
            if let Some(local) = self.data.entries.iter_mut().find(|e| e.uid == o.uid) {
                if content_key(local) == content_key(o) {
                    local.updated = local.updated.max(o.updated);
                    report.unchanged += 1;
                } else if (o.updated, content_key(o)) > (local.updated, content_key(local)) {
                    local.site = o.site.clone();
                    local.username = o.username.clone();
                    local.password = o.password.clone();
                    local.notes = o.notes.clone();
                    local.category = o.category.clone();
                    local.totp = o.totp.clone();
                    local.updated = o.updated;
                    report.updated += 1;
                } else {
                    report.unchanged += 1;
                }
                continue;
            }

            // An identical entry that only has a different id (for example both
            // devices imported the same CSV)? Treat it as the same entry, and
            // let both sides settle on the smaller id.
            if let Some(local) = self
                .data
                .entries
                .iter_mut()
                .find(|e| content_key(e) == content_key(o))
            {
                if o.uid < local.uid {
                    local.uid = o.uid.clone();
                }
                local.updated = local.updated.max(o.updated);
                report.unchanged += 1;
                continue;
            }

            // New to us, unless it was deleted after its last edit.
            if let Some(&at) = tombs.get(&o.uid) {
                if at >= o.updated {
                    continue;
                }
            }
            let id = self.data.next_id;
            self.data.next_id += 1;
            let mut copy = o.clone();
            copy.id = id;
            self.data.entries.push(copy);
            report.added += 1;
        }

        // 3. Apply deletions: remove entries whose deletion is at least as new
        //    as their last edit.
        let before = self.data.entries.len();
        self.data.entries.retain(|e| match tombs.get(&e.uid) {
            Some(&at) => at < e.updated,
            None => true,
        });
        report.deleted = (before - self.data.entries.len()) as u32;

        // 4. Keep the deletion records, except those for entries that were
        //    edited again afterwards.
        let live: HashMap<&str, u64> = self
            .data
            .entries
            .iter()
            .map(|e| (e.uid.as_str(), e.updated))
            .collect();
        let mut kept: Vec<Tombstone> = tombs
            .into_iter()
            .filter(|(uid, at)| live.get(uid.as_str()).map_or(true, |&upd| *at >= upd))
            .map(|(uid, at)| Tombstone { uid, at })
            .collect();
        kept.sort_by(|a, b| a.uid.cmp(&b.uid));
        self.data.deleted = kept;

        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::KdfParams;

    const FAST: KdfParams = KdfParams { m_cost_kib: 8, t_cost: 1, p_cost: 1 };

    fn new_vault() -> Vault {
        Vault::create("pw", FAST).unwrap()
    }

    /// A second device that starts as an exact copy of `v`.
    fn clone_vault(v: &Vault) -> Vault {
        Vault::open(&v.to_bytes().unwrap(), "pw").unwrap()
    }

    /// What a vault holds, in a form that can be compared between devices.
    fn snapshot(v: &Vault) -> Vec<(String, (String, String, String, String, String, String))> {
        let mut s: Vec<_> = v
            .entries()
            .iter()
            .map(|e| (e.uid.clone(), content_key(e)))
            .collect();
        s.sort();
        s
    }

    #[test]
    fn entries_only_on_the_other_side_are_added() {
        let mut a = new_vault();
        a.add_entry("x.com", "me", "x-pass", "");
        let mut b = new_vault();
        b.add_entry("y.com", "me", "y-pass", "");

        let r = a.merge_from(&b);
        assert_eq!(r, MergeReport { added: 1, updated: 0, deleted: 0, unchanged: 0 });
        assert_eq!(a.entries().len(), 2);
    }

    #[test]
    fn merging_twice_changes_nothing() {
        let mut a = new_vault();
        a.add_entry("x.com", "me", "x-pass", "");
        let mut b = new_vault();
        b.add_entry("y.com", "me", "y-pass", "");

        a.merge_from(&b);
        let r = a.merge_from(&b);
        assert_eq!((r.added, r.updated, r.deleted), (0, 0, 0));
        assert_eq!(a.entries().len(), 2);
    }

    #[test]
    fn both_sides_end_up_identical() {
        let mut a = new_vault();
        a.add_entry("x.com", "me", "x-pass", "");
        let mut b = new_vault();
        b.add_entry("y.com", "me", "y-pass", "");

        a.merge_from(&b);
        b.merge_from(&a);
        assert_eq!(snapshot(&a), snapshot(&b));
        assert_eq!(a.entries().len(), 2);
    }

    #[test]
    fn the_newer_edit_wins_on_both_sides() {
        let mut a = new_vault();
        a.add_entry("x.com", "me", "original", "");
        let mut b = clone_vault(&a);

        a.data.entries[0].password = "from-A".into();
        a.data.entries[0].updated = 100;
        b.data.entries[0].password = "from-B".into();
        b.data.entries[0].updated = 200;

        let r = a.merge_from(&b);
        assert_eq!(r.updated, 1);
        b.merge_from(&a);
        assert_eq!(a.entries()[0].password, "from-B");
        assert_eq!(snapshot(&a), snapshot(&b));
    }

    #[test]
    fn exact_ties_are_settled_the_same_way_on_both_sides() {
        let mut a = new_vault();
        a.add_entry("x.com", "me", "original", "");
        let mut b = clone_vault(&a);

        a.data.entries[0].password = "aaa".into();
        a.data.entries[0].updated = 100;
        b.data.entries[0].password = "zzz".into();
        b.data.entries[0].updated = 100;

        a.merge_from(&b);
        b.merge_from(&a);
        assert_eq!(a.entries()[0].password, b.entries()[0].password);
        assert_eq!(snapshot(&a), snapshot(&b));
    }

    #[test]
    fn deletions_travel_to_the_other_device() {
        let mut a = new_vault();
        let id = a.add_entry("x.com", "me", "pw", "");
        a.add_entry("keep.com", "me", "pw2", "");
        let mut b = clone_vault(&a);

        b.delete_entry(id).unwrap();
        let r = a.merge_from(&b);
        assert_eq!(r.deleted, 1);
        assert_eq!(a.entries().len(), 1);
        assert_eq!(a.entries()[0].site, "keep.com");
    }

    #[test]
    fn deleted_entries_do_not_come_back() {
        let mut a = new_vault();
        let id = a.add_entry("x.com", "me", "pw", "");
        let b = clone_vault(&a); // still has the entry

        a.delete_entry(id).unwrap();
        a.merge_from(&b);
        assert!(a.entries().is_empty(), "a deleted entry was resurrected");
    }

    #[test]
    fn an_edit_made_after_a_deletion_survives() {
        let mut a = new_vault();
        let id = a.add_entry("x.com", "me", "pw", "");
        let mut b = clone_vault(&a);

        // B deletes at time 100; A edits later, at time 200.
        b.delete_entry(id).unwrap();
        b.data.deleted[0].at = 100;
        a.data.entries[0].password = "edited-later".into();
        a.data.entries[0].updated = 200;

        a.merge_from(&b);
        assert_eq!(a.entries().len(), 1);
        assert_eq!(a.entries()[0].password, "edited-later");
        assert!(a.data.deleted.is_empty());

        // And the entry returns to B as well.
        let r = b.merge_from(&a);
        assert_eq!(r.added, 1);
        assert_eq!(snapshot(&a), snapshot(&b));
    }

    #[test]
    fn identical_entries_with_different_ids_are_not_duplicated() {
        let mut a = new_vault();
        a.add_entry("x.com", "me", "same-pass", "");
        let mut b = new_vault();
        b.add_entry("x.com", "me", "same-pass", "");
        assert_ne!(a.entries()[0].uid, b.entries()[0].uid);

        let r = a.merge_from(&b);
        assert_eq!((r.added, r.unchanged), (0, 1));
        assert_eq!(a.entries().len(), 1);

        b.merge_from(&a);
        assert_eq!(a.entries()[0].uid, b.entries()[0].uid); // both settled on one id
        assert_eq!(snapshot(&a), snapshot(&b));
    }

    #[test]
    fn old_entries_without_ids_line_up_between_devices() {
        // Two vaults from before sync existed: same entry, no ids saved yet.
        let mut a = new_vault();
        a.add_entry("x.com", "me", "pw", "");
        let mut b = new_vault();
        b.add_entry("x.com", "me", "pw", "");
        for v in [&mut a, &mut b] {
            v.data.entries[0].uid = String::new();
            v.data.entries[0].updated = 0;
            v.data.normalize_uids();
        }
        assert_eq!(a.entries()[0].uid, b.entries()[0].uid);

        let r = a.merge_from(&b);
        assert_eq!((r.added, r.unchanged), (0, 1));
    }

    #[test]
    fn merged_entries_get_fresh_local_numbers() {
        let mut a = new_vault();
        a.add_entry("a1.com", "me", "p1", "");
        a.add_entry("a2.com", "me", "p2", "");
        let mut b = new_vault();
        b.add_entry("b1.com", "me", "p3", "");
        b.add_entry("b2.com", "me", "p4", "");

        a.merge_from(&b);
        let mut ids: Vec<u64> = a.entries().iter().map(|e| e.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 4, "local numbers collided");
    }

    #[test]
    fn merge_results_and_deletion_records_survive_save_and_reopen() {
        let mut a = new_vault();
        let id = a.add_entry("x.com", "me", "pw", "");
        let b = clone_vault(&a);
        a.delete_entry(id).unwrap();
        a.merge_from(&b);

        let again = clone_vault(&a);
        assert!(again.entries().is_empty());
        assert_eq!(again.data.deleted.len(), 1);
    }

    #[test]
    fn category_and_two_factor_changes_are_merged_too() {
        let mut a = new_vault();
        let id = a.add_entry("x.com", "me", "pw", "");
        let mut b = clone_vault(&a);

        b.set_category(id, "bank").unwrap();
        b.set_totp(id, "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ").unwrap();
        b.data.entries[0].updated = a.data.entries[0].updated + 1000;

        a.merge_from(&b);
        assert_eq!(a.entries()[0].category, "bank");
        assert!(a.entries()[0].totp.is_some());
    }
}