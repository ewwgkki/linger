//! Play queue — mirrors the web version's `player` object (queue/order/i), plus repeat modes.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Repeat {
    All,
    One,
    Off,
}

impl Repeat {
    pub fn as_str(self) -> &'static str {
        match self {
            Repeat::All => "all",
            Repeat::One => "one",
            Repeat::Off => "off",
        }
    }
    pub fn parse(s: &str) -> Option<Repeat> {
        match s {
            "all" => Some(Repeat::All),
            "one" => Some(Repeat::One),
            "off" => Some(Repeat::Off),
            _ => None,
        }
    }
}

/// Which list of the queue view an operation targets.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Part {
    /// the user queue
    Up,
    /// the rest of the play source
    Ctx,
}

impl Part {
    pub fn parse(s: &str) -> Option<Part> {
        match s {
            "up" => Some(Part::Up),
            "ctx" => Some(Part::Ctx),
            _ => None,
        }
    }
}

/// The saved form of a queue (modes are stored separately).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueueSnapshot {
    pub up: Vec<String>,
    pub extra: Option<String>,
    pub items: Vec<String>,
    pub order: Vec<usize>,
    pub pos: usize,
}

pub struct Queue {
    /// user queue, played before the rest of the source
    up: Vec<String>,
    /// the song playing right now came from the user queue
    extra: Option<String>,
    items: Vec<String>,
    /// indices into `items` in playback order
    order: Vec<usize>,
    /// index into `order`
    pos: usize,
    shuffle: bool,
    repeat: Repeat,
    rng: u64,
}

impl Queue {
    pub fn new(seed: u64) -> Queue {
        // xorshift64* needs a non-zero state
        let rng = seed ^ 0x9E37_79B9_7F4A_7C15;
        Queue { up: Vec::new(), extra: None, items: Vec::new(), order: Vec::new(), pos: 0, shuffle: false, repeat: Repeat::Off, rng: rng.max(1) }
    }

    /// Like the web version: an empty list is ignored; `shuffle = true` turns shuffle on,
    /// `false` keeps the current mode.
    pub fn play_list(&mut self, items: Vec<String>, start: usize, shuffle: bool) {
        if items.is_empty() {
            return;
        }
        let start = if start < items.len() { start } else { 0 };
        self.extra = None;
        self.items = items;
        if shuffle {
            self.shuffle = true;
        }
        self.build_order(start);
        self.pos = 0;
    }

    pub fn current(&self) -> Option<&str> {
        if let Some(x) = &self.extra {
            return Some(x);
        }
        self.ctx_current()
    }

    fn ctx_current(&self) -> Option<&str> {
        self.order.get(self.pos).and_then(|&i| self.items.get(i)).map(String::as_str)
    }

    pub fn snapshot(&self) -> QueueSnapshot {
        QueueSnapshot { up: self.up.clone(), extra: self.extra.clone(), items: self.items.clone(), order: self.order.clone(), pos: self.pos }
    }

    /// Restores a saved queue without songs that are gone. Damaged data leaves the queue as it
    /// is. Returns whether there is a current song.
    pub fn restore(&mut self, s: QueueSnapshot, exists: &dyn Fn(&str) -> bool) -> bool {
        let n = s.items.len();
        let mut seen = vec![false; n];
        let whole = s.order.len() == n
            && s.order.iter().all(|&o| o < n && !std::mem::replace(&mut seen[o], true))
            && (n == 0 || s.pos < n);
        if !whole {
            return false;
        }
        let mut new_index = vec![None; n];
        let mut items = Vec::with_capacity(n);
        for (i, id) in s.items.into_iter().enumerate() {
            if exists(&id) {
                new_index[i] = Some(items.len());
                items.push(id);
            }
        }
        let kept_before = s.order[..s.pos.min(n)].iter().filter(|&&o| new_index[o].is_some()).count();
        let order: Vec<usize> = s.order.iter().filter_map(|&o| new_index[o]).collect();
        self.pos = kept_before.min(order.len().saturating_sub(1));
        self.items = items;
        self.order = order;
        self.up = s.up.into_iter().filter(|id| exists(id)).collect();
        self.extra = s.extra.filter(|id| exists(id));
        if self.current().is_none() && !self.up.is_empty() {
            self.extra = Some(self.up.remove(0));
        }
        self.current().is_some()
    }

    /// Follows songs whose ids changed because their files were renamed.
    pub fn remap(&mut self, moved: &HashMap<String, String>) {
        for id in self.up.iter_mut().chain(self.extra.iter_mut()).chain(self.items.iter_mut()) {
            if let Some(new) = moved.get(id) {
                *id = new.clone();
            }
        }
    }

    /// `auto`: the previous song ended by itself (vs. the user pressing next).
    pub fn next(&mut self, auto: bool) -> Option<&str> {
        if auto && self.repeat == Repeat::One && self.current().is_some() {
            return self.current();
        }
        if !self.up.is_empty() {
            self.extra = Some(self.up.remove(0));
            return self.current();
        }
        if self.items.is_empty() {
            return None;
        }
        if self.pos + 1 < self.order.len() {
            self.pos += 1;
        } else if self.repeat == Repeat::All || !auto {
            self.pos = 0;
        } else {
            return None; // an extra song stays current
        }
        self.extra = None;
        self.current()
    }

    /// Previous song, or the current one again when already at the first.
    pub fn prev(&mut self) -> Option<&str> {
        if self.extra.take().is_some() {
            return self.current();
        }
        if self.pos > 0 {
            self.pos -= 1;
        }
        self.current()
    }

    /// On: reshuffle with the current song first. Off: original order from the current song on.
    pub fn set_shuffle(&mut self, on: bool) {
        if self.shuffle == on {
            return;
        }
        self.shuffle = on;
        if let Some(&cur) = self.order.get(self.pos) {
            self.build_order(cur);
            self.pos = 0;
        }
    }

    pub fn set_repeat(&mut self, r: Repeat) {
        self.repeat = r;
    }

    pub fn add_next(&mut self, ids: Vec<String>) {
        self.up.splice(0..0, ids);
    }

    pub fn add_last(&mut self, ids: Vec<String>) {
        self.up.extend(ids);
    }

    pub fn up_next(&self) -> &[String] {
        &self.up
    }

    /// The rest of the source after the current position: first `limit` songs and the total.
    pub fn upcoming(&self, limit: usize) -> (Vec<String>, usize) {
        let rest = self.order.get(self.pos + 1..).unwrap_or(&[]);
        let v = rest.iter().take(limit).filter_map(|&i| self.items.get(i).cloned()).collect();
        (v, rest.len())
    }

    /// The songs right before and after the current one.
    pub fn around(&self) -> Vec<String> {
        let mut out = Vec::with_capacity(2);
        if self.extra.is_none() && self.pos > 0 {
            out.extend(self.order.get(self.pos - 1).and_then(|&i| self.items.get(i)).cloned());
        }
        match self.up.first() {
            Some(id) => out.push(id.clone()),
            None => out.extend(self.order.get(self.pos + 1).and_then(|&i| self.items.get(i)).cloned()),
        }
        out
    }

    /// The id at `index` of the part, if any.
    pub fn at(&self, part: Part, index: usize) -> Option<&str> {
        match part {
            Part::Up => self.up.get(index).map(String::as_str),
            Part::Ctx => self.order.get(self.pos + 1 + index).and_then(|&i| self.items.get(i)).map(String::as_str),
        }
    }

    pub fn clear_up(&mut self) {
        self.up.clear();
    }

    /// Songs that can still be tried: the source plus the user queue.
    pub fn total_len(&self) -> usize {
        self.items.len() + self.up.len()
    }

    fn ctx_len(&self) -> usize {
        self.order.len().saturating_sub(self.pos + 1)
    }

    pub fn remove(&mut self, part: Part, index: usize) -> bool {
        match part {
            Part::Up => {
                if index >= self.up.len() {
                    return false;
                }
                self.up.remove(index);
            }
            Part::Ctx => {
                if index >= self.ctx_len() {
                    return false;
                }
                let item = self.order.remove(self.pos + 1 + index);
                self.items.remove(item);
                for o in &mut self.order {
                    if *o > item {
                        *o -= 1;
                    }
                }
            }
        }
        true
    }

    /// Moves the entry at `from` so it ends up at `to` (indices within the part).
    pub fn move_item(&mut self, part: Part, from: usize, to: usize) -> bool {
        match part {
            Part::Up => {
                if from >= self.up.len() || to >= self.up.len() {
                    return false;
                }
                let x = self.up.remove(from);
                self.up.insert(to, x);
            }
            Part::Ctx => {
                let n = self.ctx_len();
                if from >= n || to >= n {
                    return false;
                }
                let base = self.pos + 1;
                let x = self.order.remove(base + from);
                self.order.insert(base + to, x);
            }
        }
        true
    }

    /// Makes entry `index` of the part the current song.
    pub fn jump(&mut self, part: Part, index: usize) -> Option<&str> {
        match part {
            Part::Up => {
                if index >= self.up.len() {
                    return None;
                }
                self.up.drain(..index);
                self.extra = Some(self.up.remove(0));
            }
            Part::Ctx => {
                if index >= self.ctx_len() {
                    return None;
                }
                self.extra = None;
                self.pos += index + 1;
            }
        }
        self.current()
    }

    fn build_order(&mut self, start: usize) {
        let n = self.items.len();
        if self.shuffle {
            let mut idx: Vec<usize> = (0..n).collect();
            for k in (1..n).rev() {
                let j = (self.rand() % (k as u64 + 1)) as usize;
                idx.swap(k, j);
            }
            if let Some(p) = idx.iter().position(|&i| i == start) {
                idx.remove(p);
                idx.insert(0, start);
            }
            self.order = idx;
        } else {
            self.order = (start..n).chain(0..start).collect();
        }
    }

    fn rand(&mut self) -> u64 {
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub fn shuffle(&self) -> bool {
        self.shuffle
    }
    pub fn repeat(&self) -> Repeat {
        self.repeat
    }
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(items_: &[&str], order: &[usize], pos: usize) -> QueueSnapshot {
        QueueSnapshot { up: vec![], extra: None, items: items_.iter().map(|s| s.to_string()).collect(), order: order.to_vec(), pos }
    }

    #[test]
    fn snapshot_restores_order_position_and_user_queue() {
        let mut a = Queue::new(1);
        a.play_list(items(4), 1, false);
        a.next(false);
        a.add_last(vec!["x".into()]);
        let saved = a.snapshot();
        let mut b = Queue::new(2);
        assert!(b.restore(saved.clone(), &|_| true));
        assert_eq!(b.snapshot(), saved);
        assert_eq!(b.current(), a.current());
        assert_eq!(b.next(false), Some("x"));
    }

    #[test]
    fn restore_drops_missing_songs() {
        let mut q = Queue::new(1);
        assert!(q.restore(snap(&["a", "b", "c", "d"], &[0, 1, 2, 3], 1), &|id| id != "b"));
        assert_eq!(q.current(), Some("c"));
        assert_eq!(q.upcoming(9).0, vec!["d".to_string()]);
    }

    #[test]
    fn restore_when_the_last_song_is_gone_uses_the_one_before() {
        let mut q = Queue::new(1);
        assert!(q.restore(snap(&["a", "b"], &[0, 1], 1), &|id| id != "b"));
        assert_eq!(q.current(), Some("a"));
    }

    #[test]
    fn restore_falls_back_to_the_user_queue() {
        let mut q = Queue::new(1);
        let s = QueueSnapshot { up: vec!["u".into()], ..snap(&["a"], &[0], 0) };
        assert!(q.restore(s, &|id| id == "u"));
        assert_eq!(q.current(), Some("u"));
    }

    #[test]
    fn restore_rejects_damaged_data() {
        for s in [snap(&["a", "b"], &[0], 0), snap(&["a", "b"], &[0, 0], 0), snap(&["a", "b"], &[0, 5], 0), snap(&["a", "b"], &[0, 1], 2)] {
            let mut q = Queue::new(1);
            assert!(!q.restore(s, &|_| true));
            assert!(q.is_empty());
        }
    }

    #[test]
    fn remap_follows_renamed_songs() {
        let mut q = Queue::new(1);
        q.play_list(vec!["a".into(), "b".into()], 0, false);
        q.add_last(vec!["b".into()]);
        q.remap(&HashMap::from([("b".to_string(), "n".to_string())]));
        assert_eq!(q.upcoming(5).0, vec!["n".to_string()]);
        assert_eq!(q.up_next(), ["n".to_string()]);
    }

    fn items(n: usize) -> Vec<String> {
        (0..n).map(|i| ((b'a' + i as u8) as char).to_string()).collect()
    }
    fn walk(q: &mut Queue, auto: bool, n: usize) -> Vec<String> {
        let mut out = vec![q.current().unwrap().to_string()];
        for _ in 1..n {
            out.push(q.next(auto).unwrap().to_string());
        }
        out
    }

    #[test]
    fn sequential_order_rotates_from_start() {
        let mut q = Queue::new(1);
        q.set_repeat(Repeat::Off);
        q.play_list(items(5), 2, false);
        assert_eq!(walk(&mut q, true, 5), ["c", "d", "e", "a", "b"]);
        assert_eq!(q.next(true), None);
        // stopping leaves the last song current
        assert_eq!(q.current(), Some("b"));
    }

    #[test]
    fn manual_next_wraps_even_when_repeat_off() {
        let mut q = Queue::new(1);
        q.set_repeat(Repeat::Off);
        q.play_list(items(3), 0, false);
        q.next(false);
        q.next(false);
        assert_eq!(q.current(), Some("c"));
        assert_eq!(q.next(false), Some("a"));
    }

    #[test]
    fn repeat_one_auto_repeats_manual_advances() {
        let mut q = Queue::new(1);
        q.set_repeat(Repeat::One);
        q.play_list(items(3), 1, false);
        assert_eq!(q.next(true), Some("b"));
        assert_eq!(q.next(true), Some("b"));
        assert_eq!(q.next(false), Some("c"));
        assert_eq!(q.next(false), Some("a"));
    }

    #[test]
    fn repeat_all_wraps_on_auto() {
        let mut q = Queue::new(1);
        q.set_repeat(Repeat::All);
        q.play_list(items(3), 0, false);
        assert_eq!(walk(&mut q, true, 7), ["a", "b", "c", "a", "b", "c", "a"]);
    }

    #[test]
    fn prev_goes_back_or_stays_on_first() {
        let mut q = Queue::new(1);
        q.play_list(items(3), 1, false);
        assert_eq!(q.prev(), Some("b"));
        q.next(false);
        assert_eq!(q.prev(), Some("b"));
        assert_eq!(Queue::new(1).prev(), None);
    }

    #[test]
    fn shuffle_puts_start_first_and_covers_all() {
        for seed in [1u64, 7, 42, 0xdead_beef] {
            let mut q = Queue::new(seed);
            q.set_repeat(Repeat::Off);
            q.play_list(items(20), 13, true);
            assert!(q.shuffle());
            let mut got = walk(&mut q, true, 20);
            assert_eq!(got[0], "n");
            assert_eq!(q.next(true), None);
            got.sort();
            assert_eq!(got, items(20));
        }
        // the order is actually shuffled (deterministic per seed)
        let order = |seed| {
            let mut q = Queue::new(seed);
            q.play_list(items(20), 0, true);
            walk(&mut q, true, 20)
        };
        assert_eq!(order(5), order(5));
        assert_ne!(order(5), items(20));
    }

    #[test]
    fn toggling_shuffle_keeps_current() {
        let mut q = Queue::new(3);
        q.set_repeat(Repeat::Off);
        q.play_list(items(10), 0, false);
        q.next(false);
        q.next(false); // "c"
        q.set_shuffle(true);
        assert_eq!(q.current(), Some("c"));
        let mut got = walk(&mut q, true, 10);
        assert_eq!(q.next(true), None);
        got.sort();
        assert_eq!(got, items(10));

        // turning it off continues from the current song in original order
        q.play_list(items(10), 4, true);
        q.next(false);
        let cur = q.current().unwrap().to_string();
        q.set_shuffle(false);
        assert!(!q.shuffle());
        assert_eq!(q.current(), Some(cur.as_str()));
        let i = items(10).iter().position(|s| *s == cur).unwrap();
        let expect: Vec<String> = (0..10).map(|k| items(10)[(i + k) % 10].clone()).collect();
        assert_eq!(walk(&mut q, true, 10), expect);
    }

    #[test]
    fn empty_and_out_of_range() {
        let mut q = Queue::new(0);
        assert_eq!(q.current(), None);
        assert_eq!(q.next(false), None);
        assert_eq!(q.next(true), None);
        assert_eq!(q.prev(), None);
        q.set_shuffle(true);
        q.play_list(vec![], 0, false);
        assert_eq!(q.current(), None);
        q.play_list(items(3), 99, false);
        assert_eq!(q.current(), Some("a"));
    }

    #[test]
    fn repeat_round_trips() {
        for r in [Repeat::All, Repeat::One, Repeat::Off] {
            assert_eq!(Repeat::parse(r.as_str()), Some(r));
        }
        assert_eq!(Repeat::parse("x"), None);
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn user_queue_plays_before_context() {
        let mut q = Queue::new(1);
        q.play_list(items(3), 0, false);
        q.add_last(s(&["x"]));
        q.add_next(s(&["y", "z"]));
        assert_eq!(q.up_next(), ["y", "z", "x"]);
        assert_eq!(q.next(false), Some("y"));
        assert_eq!(q.next(false), Some("z"));
        assert_eq!(q.next(false), Some("x"));
        assert_eq!(q.next(false), Some("b"));
    }

    #[test]
    fn prev_from_user_queue_returns_context_song() {
        let mut q = Queue::new(1);
        q.play_list(items(3), 0, false);
        q.add_next(s(&["x"]));
        assert_eq!(q.next(false), Some("x"));
        assert_eq!(q.prev(), Some("a"));
    }

    #[test]
    fn repeat_one_replays_user_queue_song() {
        let mut q = Queue::new(1);
        q.play_list(items(3), 0, false);
        q.set_repeat(Repeat::One);
        q.add_next(s(&["x"]));
        assert_eq!(q.next(false), Some("x"));
        assert_eq!(q.next(true), Some("x"));
        assert_eq!(q.next(false), Some("b"));
    }

    #[test]
    fn play_list_keeps_user_queue() {
        let mut q = Queue::new(1);
        q.play_list(items(3), 0, false);
        q.add_next(s(&["x"]));
        q.play_list(s(&["d", "e"]), 0, false);
        assert_eq!(q.current(), Some("d"));
        assert_eq!(q.up_next(), ["x"]);
    }

    #[test]
    fn upcoming_limit_and_total() {
        let mut q = Queue::new(1);
        let many: Vec<String> = (0..1000).map(|i| format!("s{i}")).collect();
        q.play_list(many, 0, false);
        let (v, total) = q.upcoming(200);
        assert_eq!(v.len(), 200);
        assert_eq!(v[0], "s1");
        assert_eq!(total, 999);
    }

    #[test]
    fn remove_ctx_survives_shuffle_toggle() {
        let mut q = Queue::new(1);
        q.play_list(items(4), 0, false);
        assert!(q.remove(Part::Ctx, 1));
        assert_eq!(q.upcoming(10).0, ["b", "d"]);
        q.set_shuffle(true);
        q.set_shuffle(false);
        let (v, total) = q.upcoming(10);
        assert!(!v.contains(&"c".to_string()));
        assert_eq!((v.len(), total), (2, 2));
        assert_eq!(q.len(), 3);
    }

    #[test]
    fn ops_target_visible_order_with_shuffle() {
        let mut q = Queue::new(9);
        q.play_list(items(10), 0, true);
        let v = q.upcoming(10).0;
        assert!(q.remove(Part::Ctx, 2));
        let mut e = v.clone();
        e.remove(2);
        assert_eq!(q.upcoming(10).0, e);
        assert!(q.move_item(Part::Ctx, 0, 3));
        let first = e.remove(0);
        e.insert(3, first);
        assert_eq!(q.upcoming(10).0, e);
        assert_eq!(q.jump(Part::Ctx, 1), Some(e[1].as_str()));
        assert_eq!(q.current(), Some(e[1].as_str()));
    }

    #[test]
    fn jump_up_drops_skipped() {
        let mut q = Queue::new(1);
        q.play_list(items(2), 0, false);
        q.add_last(s(&["x", "y", "z"]));
        assert_eq!(q.jump(Part::Up, 1), Some("y"));
        assert_eq!(q.current(), Some("y"));
        assert_eq!(q.up_next(), ["z"]);
    }

    #[test]
    fn bad_indices_are_noops() {
        let mut q = Queue::new(1);
        q.play_list(items(3), 0, false);
        q.add_last(s(&["x"]));
        assert!(!q.remove(Part::Ctx, 2));
        assert!(!q.remove(Part::Up, 1));
        assert!(!q.move_item(Part::Ctx, 0, 2));
        assert!(!q.move_item(Part::Up, 1, 0));
        assert_eq!(q.jump(Part::Ctx, 2), None);
        assert_eq!(q.jump(Part::Up, 1), None);
        assert_eq!(q.current(), Some("a"));
        assert_eq!(q.up_next(), ["x"]);
        assert_eq!(q.upcoming(10).0, ["b", "c"]);
        assert_eq!(q.len(), 3);
    }

    #[test]
    fn at_clear_up_and_total_len() {
        let mut q = Queue::new(1);
        q.play_list(items(3), 1, false);
        q.add_last(s(&["x", "y"]));
        assert_eq!(q.at(Part::Up, 1), Some("y"));
        assert_eq!(q.at(Part::Up, 2), None);
        assert_eq!(q.at(Part::Ctx, 0), Some("c"));
        assert_eq!(q.at(Part::Ctx, 1), Some("a"));
        assert_eq!(q.at(Part::Ctx, 2), None);
        assert_eq!(q.total_len(), 5);
        q.clear_up();
        assert!(q.up_next().is_empty());
        assert_eq!(q.total_len(), 3);
        assert_eq!(q.current(), Some("b"));
    }

    #[test]
    fn part_parses() {
        assert_eq!(Part::parse("up"), Some(Part::Up));
        assert_eq!(Part::parse("ctx"), Some(Part::Ctx));
        assert_eq!(Part::parse("x"), None);
    }
}
