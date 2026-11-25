//! The alias table (contract §2.8): `c<n>` cards, `l<n>` links, `f<n>` frames, `m<n>` marks.
//! Engine-only writer; numbers are never reused, so an alias is the same for every session forever.

use std::collections::{BTreeMap, HashMap};

use yrs::{Map, ReadTxn, TransactionMut};

use super::model::Maps;

/// `(root map, prefix)` of every aliased kind.
pub const KINDS: [(&str, char); 4] = [("cards", 'c'), ("links", 'l'), ("frames", 'f'), ("marks", 'm')];

/// The aliases `ensure` would add: for each kind, entries without one get `prefix + (max + 1)`
/// in ULID order. Pure over the current keys and table.
pub fn missing<T: ReadTxn>(maps: &Maps, txn: &T) -> Vec<(String, String)> {
    let table: BTreeMap<String, String> = maps
        .aliases
        .iter(txn)
        .filter_map(|(k, v)| match v {
            yrs::Out::Any(yrs::Any::String(s)) => Some((k.to_owned(), s.to_string())),
            _ => None,
        })
        .collect();
    let mut max: HashMap<char, u32> = HashMap::new();
    for a in table.values() {
        let mut ch = a.chars();
        if let Some(p) = ch.next()
            && let Ok(n) = ch.as_str().parse::<u32>()
        {
            let m = max.entry(p).or_default();
            *m = (*m).max(n);
        }
    }
    let mut out = Vec::new();
    for (root, prefix) in KINDS {
        let map = match root {
            "cards" => &maps.cards,
            "links" => &maps.links,
            "frames" => &maps.frames,
            _ => &maps.marks,
        };
        let mut keys: Vec<String> = map
            .keys(txn)
            .filter(|k| !table.contains_key(*k))
            .map(str::to_owned)
            .collect();
        keys.sort();
        let n = max.entry(prefix).or_default();
        for k in keys {
            *n += 1;
            out.push((k, format!("{prefix}{n}")));
        }
    }
    out
}

/// Write every missing alias in `txn`; returns how many were added.
pub fn ensure(maps: &Maps, txn: &mut TransactionMut) -> usize {
    let add = missing(maps, txn);
    for (id, alias) in &add {
        maps.set_alias(txn, id, alias);
    }
    add.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wb::model::{CardKind, card_fields, read, tests::card};
    use yrs::updates::decoder::Decode;
    use yrs::{Doc, Transact};

    #[test]
    fn stable_and_never_reused() {
        let doc = Doc::new();
        let maps = Maps::of(&doc);
        let put = |id: &str| {
            let mut txn = doc.transact_mut();
            maps.put(&mut txn, "cards", id, card_fields(&card(id, CardKind::Sticky, [0.0; 4], "t", vec![])));
        };
        put("01B");
        put("01A");
        {
            let mut txn = doc.transact_mut();
            assert_eq!(ensure(&maps, &mut txn), 2);
            // Idempotent.
            assert_eq!(ensure(&maps, &mut txn), 0);
        }
        let a = read(&doc.transact()).aliases;
        assert_eq!(a["01A"], "c1");
        assert_eq!(a["01B"], "c2");
        // Delete c2's card: its number stays reserved, the next card is c3.
        {
            let mut txn = doc.transact_mut();
            maps.delete(&mut txn, "cards", "01B");
        }
        put("01C");
        {
            let mut txn = doc.transact_mut();
            ensure(&maps, &mut txn);
        }
        let a = read(&doc.transact()).aliases;
        assert_eq!(a["01A"], "c1");
        assert_eq!(a["01B"], "c2");
        assert_eq!(a["01C"], "c3");
        // A second engine reading the same state computes the same table (it is in the document).
        let other = Doc::new();
        let u = doc.transact().encode_state_as_update_v1(&Default::default());
        other.transact_mut().apply_update(yrs::Update::decode_v1(&u).unwrap()).unwrap();
        assert_eq!(read(&other.transact()).aliases, a);
        let om = Maps::of(&other);
        assert!(missing(&om, &other.transact()).is_empty());
    }
}
