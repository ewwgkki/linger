//! Play queue — mirrors the web version's `player` object (queue/order/i), plus repeat modes.

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

pub struct Queue {
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
        Queue { items: Vec::new(), order: Vec::new(), pos: 0, shuffle: false, repeat: Repeat::Off, rng: rng.max(1) }
    }

    /// Like the web version: an empty list is ignored; `shuffle = true` turns shuffle on,
    /// `false` keeps the current mode.
    pub fn play_list(&mut self, items: Vec<String>, start: usize, shuffle: bool) {
        if items.is_empty() {
            return;
        }
        let start = if start < items.len() { start } else { 0 };
        self.items = items;
        if shuffle {
            self.shuffle = true;
        }
        self.build_order(start);
        self.pos = 0;
    }

    pub fn current(&self) -> Option<&str> {
        self.order.get(self.pos).and_then(|&i| self.items.get(i)).map(String::as_str)
    }

    /// `auto`: the previous song ended by itself (vs. the user pressing next).
    pub fn next(&mut self, auto: bool) -> Option<&str> {
        if self.items.is_empty() {
            return None;
        }
        if !(auto && self.repeat == Repeat::One) {
            if self.pos + 1 < self.order.len() {
                self.pos += 1;
            } else if self.repeat == Repeat::All || !auto {
                self.pos = 0;
            } else {
                return None;
            }
        }
        self.current()
    }

    /// Previous song, or the current one again when already at the first.
    pub fn prev(&mut self) -> Option<&str> {
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
}
