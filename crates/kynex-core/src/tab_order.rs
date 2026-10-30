//! Reordering project tabs: moving one, where the others' indices end up, and
//! which slot a dragged tab is over.

/// Moves the item at `from` to `to`, shifting the ones between. Out of range does nothing.
pub fn move_item<T>(items: &mut Vec<T>, from: usize, to: usize) {
    if from < items.len() && to < items.len() {
        let item = items.remove(from);
        items.insert(to, item);
    }
}

/// Where the item at `i` is after the one at `from` moves to `to`.
pub fn moved(i: usize, from: usize, to: usize) -> usize {
    if i == from {
        to
    } else if from < i && i <= to {
        i - 1
    } else if to <= i && i < from {
        i + 1
    } else {
        i
    }
}

/// The items' original indices in the order shown while the one at `from`
/// is held over slot `to`.
pub fn preview(len: usize, from: usize, to: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..len).collect();
    move_item(&mut order, from, to);
    order
}

/// The slot a dragged tab goes to with the pointer at `x`. `at` is the slot it
/// is shown in now and `spans` the (left, right) of every tab as shown, the
/// dragged one included. It passes a neighbour once the pointer crosses that
/// neighbour's middle; the tabs beyond don't move when it does, so it can pass
/// several at once without settling back.
pub fn slot(at: usize, spans: &[(f32, f32)], x: f32) -> usize {
    let mid = |i: usize| (spans[i].0 + spans[i].1) / 2.;
    let mut at = at.min(spans.len().saturating_sub(1));
    while at > 0 && x < mid(at - 1) {
        at -= 1;
    }
    while at + 1 < spans.len() && x > mid(at + 1) {
        at += 1;
    }
    at
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moves_either_way_and_ignores_bad_indices() {
        let mut v = vec!['a', 'b', 'c', 'd'];
        move_item(&mut v, 0, 2);
        assert_eq!(v, ['b', 'c', 'a', 'd']);
        move_item(&mut v, 3, 0);
        assert_eq!(v, ['d', 'b', 'c', 'a']);
        move_item(&mut v, 1, 1);
        assert_eq!(v, ['d', 'b', 'c', 'a']);
        move_item(&mut v, 4, 0);
        move_item(&mut v, 0, 4);
        assert_eq!(v, ['d', 'b', 'c', 'a']);
    }

    #[test]
    fn indices_follow_the_move() {
        // Every index lands where `move_item` puts its item.
        for len in 1..6 {
            for from in 0..len {
                for to in 0..len {
                    let mut v: Vec<usize> = (0..len).collect();
                    move_item(&mut v, from, to);
                    for i in 0..len {
                        assert_eq!(v[moved(i, from, to)], i, "len {len}, {from} → {to}, item {i}");
                    }
                    assert_eq!(preview(len, from, to), v);
                }
            }
        }
    }

    #[test]
    fn slots_change_at_a_neighbours_middle() {
        // Three tabs of different widths: 0..40, 40..120, 120..150.
        let spans = [(0., 40.), (40., 120.), (120., 150.)];
        // Holding the first: stays until past the middle of the second (80).
        assert_eq!(slot(0, &spans, 79.), 0);
        assert_eq!(slot(0, &spans, 81.), 1);
        // Past the third's middle too: both at once.
        assert_eq!(slot(0, &spans, 140.), 2);
        // Holding the last: back over the others, again at their middles.
        assert_eq!(slot(2, &spans, 81.), 2);
        assert_eq!(slot(2, &spans, 79.), 1);
        assert_eq!(slot(2, &spans, 10.), 0);
        // Off either end clamps.
        assert_eq!(slot(1, &spans, -50.), 0);
        assert_eq!(slot(1, &spans, 500.), 2);
        assert_eq!(slot(5, &spans, 500.), 2);
        assert_eq!(slot(0, &[], 10.), 0);
    }

    #[test]
    fn a_pass_doesnt_bounce_back() {
        // Tab 1 (wide) is dragged left past tab 0 (narrow): shown order becomes
        // [1, 0], and the pointer that caused it must keep it there.
        let before = [(0., 40.), (40., 140.)];
        let x = 15.;
        assert_eq!(slot(1, &before, x), 0);
        let after = [(0., 100.), (100., 140.)];
        assert_eq!(slot(0, &after, x), 0);
    }
}
