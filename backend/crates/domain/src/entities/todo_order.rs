//! Where a moved todo goes (OT-013 design D6), as integer-only kernels (D13).
//!
//! The repository resolves a move under the list lock: [`move_slot`] picks the
//! slot among the list's other open todos, [`position_between`] the position in
//! that slot, and, when the slot has no room, [`renumbered_position`] and
//! [`renumbered_other`] space the whole list out again.

use super::todo::POSITION_GAP;

/// Most neighbour IDs a move may send each way (design D6).
pub const MAX_MOVE_NEIGHBOURS: usize = 20;

/// The slot a move asks for among `others`, the list's other open todos in order.
///
/// Slot `n` is directly above `others[n]`; slot `others.len()` is the bottom.
/// `above` and `below` are nearest first, and IDs not in `others` are ignored:
/// the moved todo's own, deleted, done and foreign ones alike. An `above` sent
/// empty means the top. One whose IDs have all gone does not: the todo then goes
/// above the first remaining ID in `below`, else to the bottom.
#[must_use]
pub fn move_slot<T: PartialEq>(others: &[T], above: &[T], below: &[T]) -> usize {
    if above.is_empty() {
        return 0;
    }
    let rank = |id: &T| others.iter().position(|other| other == id);
    above
        .iter()
        .find_map(rank)
        .map(|rank| rank + 1)
        .or_else(|| below.iter().find_map(rank))
        .unwrap_or(others.len())
}

/// A position strictly between two neighbours, or `None` to ask for renumbering.
///
/// `above` is the todo directly above the slot and `below` the one directly
/// below it; `None` is an end of the list. Total over every `i64`: at either
/// end of the range, and for neighbours out of order, it asks for renumbering.
#[must_use]
pub const fn position_between(above: Option<i64>, below: Option<i64>) -> Option<i64> {
    match (above, below) {
        (None, None) => Some(POSITION_GAP),
        (Some(above), None) => above.checked_add(POSITION_GAP),
        (None, Some(below)) => below.checked_sub(POSITION_GAP),
        (Some(above), Some(below)) => {
            let middle = i64::midpoint(above, below);
            if above < middle && middle < below {
                Some(middle)
            } else {
                None
            }
        }
    }
}

/// The renumbered position of the todo at `rank` in the final order.
#[must_use]
pub fn renumbered_position(rank: u32) -> i64 {
    (i64::from(rank) + 1) * POSITION_GAP
}

/// The renumbered position of the other todo at `rank`, with the moved todo
/// taking `slot`: those from the slot down make room for it.
#[must_use]
pub fn renumbered_other(rank: u16, slot: u16) -> i64 {
    renumbered_position(u32::from(rank) + u32::from(rank >= slot))
}

/// Kani proofs (`just verify`). Bounds and assumptions are on each harness.
#[cfg(kani)]
mod verification {
    use super::*;

    /// Spec: a move lands strictly between its neighbours, or asks for
    /// renumbering. Every pair of `i64` neighbours, either or both absent, in
    /// any order; no assumptions. No overflow or panic.
    #[kani::proof]
    fn a_move_lands_strictly_between_or_asks_for_renumbering() {
        let above: Option<i64> = kani::any();
        let below: Option<i64> = kani::any();
        if let Some(position) = position_between(above, below) {
            if let Some(above) = above {
                assert!(above < position, "not below the todo above");
            }
            if let Some(below) = below {
                assert!(position < below, "not above the todo below");
            }
        }
    }

    /// Spec: between two neighbours renumbering is asked for exactly when no
    /// integer lies strictly between them. Every `i64` pair; no assumptions.
    /// Differences are taken in `i128`, not with the code's midpoint.
    #[kani::proof]
    fn renumbering_is_asked_for_only_without_room() {
        let above: i64 = kani::any();
        let below: i64 = kani::any();
        let room = i128::from(below) - i128::from(above) >= 2;
        assert_eq!(position_between(Some(above), Some(below)).is_some(), room);
    }

    /// Spec: past either end of the list the todo is 2^20 from its neighbour,
    /// and renumbering is asked for exactly where that leaves `i64`. Every
    /// `i64` neighbour; no assumptions. 2^20 is the design's literal.
    #[kani::proof]
    fn past_either_end_is_one_gap_or_renumbering() {
        let neighbour: i64 = kani::any();
        let low = i128::from(neighbour) - 1_048_576;
        let high = i128::from(neighbour) + 1_048_576;
        match position_between(None, Some(neighbour)) {
            Some(position) => assert_eq!(i128::from(position), low),
            None => assert!(low < i128::from(i64::MIN)),
        }
        match position_between(Some(neighbour), None) {
            Some(position) => assert_eq!(i128::from(position), high),
            None => assert!(high > i128::from(i64::MAX)),
        }
        assert!(position_between(None, None).is_some());
    }

    /// Spec: renumbering keeps the order and puts the moved todo in its slot,
    /// leaving room for a move between any two. Bound: 5 other todos at any
    /// strictly increasing `i64` positions; every slot 0 to 5.
    #[kani::proof]
    #[kani::unwind(7)]
    fn renumbering_keeps_the_order_of_five() {
        let old: [i64; 5] = kani::any();
        let (i, j): (u16, u16) = (kani::any(), kani::any());
        let slot: u16 = kani::any();
        kani::assume(i < 5 && j < 5 && slot <= 5);
        kani::assume(old[0] < old[1] && old[1] < old[2]);
        kani::assume(old[2] < old[3] && old[3] < old[4]);

        let (new_i, new_j) = (renumbered_other(i, slot), renumbered_other(j, slot));
        let before = old[usize::from(i)] < old[usize::from(j)];
        assert_eq!(new_i < new_j, before, "order changed");

        let moved = renumbered_position(u32::from(slot));
        assert_eq!(new_i < moved, i < slot, "moved todo not in its slot");
        assert!(new_i != moved, "two todos share a position");
        if before {
            assert!(position_between(Some(new_i), Some(new_j)).is_some());
        }
    }

    /// Spec: a renumbered list starts above zero and stays in range, for every
    /// `u16` rank and slot (a list holds at most 1,000 todos); no overflow.
    #[kani::proof]
    fn renumbered_positions_are_distinct_for_every_rank() {
        let (a, b, slot): (u16, u16, u16) = (kani::any(), kani::any(), kani::any());
        kani::assume(a < b);
        assert!(renumbered_other(a, slot) > 0);
        assert!(renumbered_other(a, slot) < renumbered_other(b, slot));
        assert!(renumbered_other(a, slot) != renumbered_position(u32::from(slot)));
    }

    /// Spec (D6): below the first remaining ID in `above`, else above the first
    /// remaining one in `below`, else the bottom; an empty `above` is the top.
    /// Bound: 3 distinct other todos, up to 2 IDs each way, any `u8` IDs
    /// (repeated and unknown ones included).
    #[kani::proof]
    #[kani::unwind(5)]
    fn a_move_takes_the_slot_its_neighbours_name() {
        let others: [u8; 3] = kani::any();
        kani::assume(others[0] != others[1]);
        kani::assume(others[0] != others[2] && others[1] != others[2]);
        let (above_ids, below_ids): ([u8; 2], [u8; 2]) = (kani::any(), kani::any());
        let (above_len, below_len): (usize, usize) = (kani::any(), kani::any());
        kani::assume(above_len <= 2 && below_len <= 2);

        let slot = move_slot(&others, &above_ids[..above_len], &below_ids[..below_len]);

        let is = |slot: usize, id: u8| slot < 3 && others[slot] == id;
        let known = |id: u8| is(0, id) || is(1, id) || is(2, id);
        assert!(slot <= 3);
        if above_len == 0 {
            assert_eq!(slot, 0);
        } else if known(above_ids[0]) {
            assert!(slot >= 1 && is(slot - 1, above_ids[0]));
        } else if above_len == 2 && known(above_ids[1]) {
            assert!(slot >= 1 && is(slot - 1, above_ids[1]));
        } else if below_len >= 1 && known(below_ids[0]) {
            assert!(is(slot, below_ids[0]));
        } else if below_len == 2 && known(below_ids[1]) {
            assert!(is(slot, below_ids[1]));
        } else {
            assert_eq!(slot, 3);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn slots_follow_the_neighbours() {
        let others = ['a', 'b', 'c'];
        for (above, below, slot) in [
            (&[][..], &['a'][..], 0),
            (&[], &['c'], 0),
            (&['a'], &['b'], 1),
            (&['x', 'b'], &['a'], 2),
            (&['c'], &[], 3),
            (&['x'], &['y', 'b'], 1),
            (&['x'], &['y'], 3),
            (&['x'], &[], 3),
        ] {
            assert_eq!(
                move_slot(&others, above, below),
                slot,
                "{above:?} {below:?}"
            );
        }
        assert_eq!(move_slot(&[], &['x'], &['y']), 0);
    }

    #[test]
    fn positions_at_the_ends_of_the_range_ask_for_renumbering() {
        assert_eq!(position_between(None, None), Some(POSITION_GAP));
        assert_eq!(position_between(Some(i64::MAX), None), None);
        assert_eq!(position_between(None, Some(i64::MIN)), None);
        assert_eq!(position_between(Some(4), Some(5)), None);
        assert_eq!(position_between(Some(-1), Some(0)), None);
        assert_eq!(position_between(Some(-1), Some(1)), Some(0));
        assert_eq!(position_between(Some(i64::MIN), Some(i64::MAX)), Some(0));
    }

    /// The whole move as the repository does it: the order after it.
    fn apply_move(positions: &[i64], slot: usize) -> Vec<(i64, bool)> {
        let above = slot.checked_sub(1).map(|rank| positions[rank]);
        let below = positions.get(slot).copied();
        let mut after: Vec<(i64, bool)> = Vec::new();
        if let Some(moved) = position_between(above, below) {
            after.extend(positions.iter().map(|position| (*position, false)));
            after.push((moved, true));
        } else {
            let slot = u16::try_from(slot).unwrap();
            for rank in 0..u16::try_from(positions.len()).unwrap() {
                after.push((renumbered_other(rank, slot), false));
            }
            after.push((renumbered_position(u32::from(slot)), true));
        }
        after.sort_unstable();
        after
    }

    proptest! {
        /// Ties the kernels together at real list sizes; Kani covers 5.
        #[test]
        fn a_move_lands_in_its_slot_in_up_to_1000_positions(
            positions in prop_oneof![
                proptest::collection::btree_set(any::<i64>(), 0..1000),
                // Adjacent positions, so that renumbering is common.
                proptest::collection::btree_set(-600i64..600, 0..1000),
                proptest::collection::btree_set((i64::MAX - 1500)..=i64::MAX, 0..1000),
                proptest::collection::btree_set(i64::MIN..(i64::MIN + 1500), 0..1000),
            ],
            slot in any::<prop::sample::Index>(),
        ) {
            let positions: Vec<i64> = positions.into_iter().collect();
            let slot = slot.index(positions.len() + 1);
            let after = apply_move(&positions, slot);
            prop_assert_eq!(after.len(), positions.len() + 1);
            prop_assert!(after.is_sorted_by(|a, b| a.0 < b.0));
            prop_assert_eq!(after.iter().position(|(_, moved)| *moved), Some(slot));
        }
    }
}
