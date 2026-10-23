//! Multiprecision decimal numbers used by the exact float formatting and parsing paths.
//!
//! Follows the behaviour of Go's standard library strconv package (The Go Authors, BSD-style licence).

/// Maximum number of digits kept; further non-zero digits only set `trunc`.
const MAX_DIGITS: usize = 800;
/// Largest shift done at once (Go's `maxShift` on 64-bit platforms).
const MAX_SHIFT: u32 = 60;

/// A decimal number `0.d[..nd] * 10^dp`.
pub(super) struct Decimal {
    /// Digits as ASCII bytes, big-endian.
    pub d: [u8; MAX_DIGITS],
    /// Number of digits used.
    pub nd: usize,
    /// Decimal point position.
    pub dp: i32,
    /// Negative flag.
    pub neg: bool,
    /// Discarded non-zero digits beyond `d[..nd]`.
    pub trunc: bool,
}

impl Decimal {
    pub fn new() -> Box<Decimal> {
        Box::new(Decimal { d: [0; MAX_DIGITS], nd: 0, dp: 0, neg: false, trunc: false })
    }

    fn trim(&mut self) {
        while self.nd > 0 && self.d[self.nd - 1] == b'0' {
            self.nd -= 1;
        }
        if self.nd == 0 {
            self.dp = 0;
        }
    }

    /// Sets the decimal to the unsigned integer `v`.
    pub fn assign(&mut self, mut v: u64) {
        let mut buf = [0u8; 24];
        let mut n = 0;
        while v > 0 {
            buf[n] = (v % 10) as u8 + b'0';
            n += 1;
            v /= 10;
        }
        self.nd = 0;
        while n > 0 {
            n -= 1;
            self.d[self.nd] = buf[n];
            self.nd += 1;
        }
        self.dp = self.nd as i32;
        self.trim();
    }

    fn right_shift(&mut self, k: u32) {
        let mut r = 0;
        let mut w = 0;
        let mut n: u64 = 0;
        while n >> k == 0 {
            if r >= self.nd {
                if n == 0 {
                    self.nd = 0;
                    return;
                }
                while n >> k == 0 {
                    n *= 10;
                    r += 1;
                }
                break;
            }
            n = n * 10 + u64::from(self.d[r] - b'0');
            r += 1;
        }
        self.dp -= r as i32 - 1;
        let mask: u64 = (1 << k) - 1;
        while r < self.nd {
            let c = u64::from(self.d[r] - b'0');
            let dig = n >> k;
            n &= mask;
            self.d[w] = dig as u8 + b'0';
            w += 1;
            n = n * 10 + c;
            r += 1;
        }
        while n > 0 {
            let dig = n >> k;
            n &= mask;
            if w < MAX_DIGITS {
                self.d[w] = dig as u8 + b'0';
                w += 1;
            } else if dig > 0 {
                self.trunc = true;
            }
            n *= 10;
        }
        self.nd = w;
        self.trim();
    }

    fn left_shift(&mut self, k: u32) {
        let (mut delta, cutoff) = LEFT_CHEATS[k as usize];
        if prefix_is_less_than(&self.d[..self.nd], cutoff.as_bytes()) {
            delta -= 1;
        }
        let mut r = self.nd;
        let mut w = self.nd + delta;
        let mut n: u64 = 0;
        let put = |w: usize, rem: u64, dec: &mut Decimal| {
            if w < MAX_DIGITS {
                dec.d[w] = rem as u8 + b'0';
            } else if rem != 0 {
                dec.trunc = true;
            }
        };
        while r > 0 {
            r -= 1;
            n += u64::from(self.d[r] - b'0') << k;
            let quo = n / 10;
            let rem = n - 10 * quo;
            w -= 1;
            put(w, rem, self);
            n = quo;
        }
        while n > 0 {
            let quo = n / 10;
            let rem = n - 10 * quo;
            w -= 1;
            put(w, rem, self);
            n = quo;
        }
        self.nd = (self.nd + delta).min(MAX_DIGITS);
        self.dp += delta as i32;
        self.trim();
    }

    /// Multiplies by `2^k` (divides when `k` is negative).
    pub fn shift(&mut self, mut k: i32) {
        if self.nd == 0 {
            return;
        }
        if k > 0 {
            while k > MAX_SHIFT as i32 {
                self.left_shift(MAX_SHIFT);
                k -= MAX_SHIFT as i32;
            }
            self.left_shift(k as u32);
        } else if k < 0 {
            while k < -(MAX_SHIFT as i32) {
                self.right_shift(MAX_SHIFT);
                k += MAX_SHIFT as i32;
            }
            self.right_shift((-k) as u32);
        }
    }

    fn should_round_up(&self, nd: i64) -> bool {
        if nd < 0 || nd >= self.nd as i64 {
            return false;
        }
        let nd = nd as usize;
        if self.d[nd] == b'5' && nd + 1 == self.nd {
            // Exactly halfway: round to even.
            if self.trunc {
                return true;
            }
            return nd > 0 && !(self.d[nd - 1] - b'0').is_multiple_of(2);
        }
        self.d[nd] >= b'5'
    }

    /// Rounds to `nd` digits (or fewer), half to even.
    pub fn round(&mut self, nd: i64) {
        if nd < 0 || nd >= self.nd as i64 {
            return;
        }
        if self.should_round_up(nd) {
            self.round_up(nd);
        } else {
            self.round_down(nd);
        }
    }

    /// Truncates to `nd` digits (or fewer).
    pub fn round_down(&mut self, nd: i64) {
        if nd < 0 || nd >= self.nd as i64 {
            return;
        }
        self.nd = nd as usize;
        self.trim();
    }

    /// Rounds up to `nd` digits (or fewer).
    pub fn round_up(&mut self, nd: i64) {
        if nd < 0 || nd >= self.nd as i64 {
            return;
        }
        let nd = nd as usize;
        for i in (0..nd).rev() {
            if self.d[i] < b'9' {
                self.d[i] += 1;
                self.nd = i + 1;
                return;
            }
        }
        self.d[0] = b'1';
        self.nd = 1;
        self.dp += 1;
    }

    /// Extracts the integer part, rounded appropriately; no guarantees about overflow.
    pub fn rounded_integer(&self) -> u64 {
        if self.dp > 20 {
            return u64::MAX;
        }
        let dp = self.dp.max(0) as usize;
        let mut n: u64 = 0;
        let mut i = 0;
        while i < dp && i < self.nd {
            n = n.wrapping_mul(10).wrapping_add(u64::from(self.d[i] - b'0'));
            i += 1;
        }
        while i < dp {
            n = n.wrapping_mul(10);
            i += 1;
        }
        if self.should_round_up(i64::from(self.dp)) {
            n = n.wrapping_add(1);
        }
        n
    }
}

fn prefix_is_less_than(b: &[u8], s: &[u8]) -> bool {
    for (i, &c) in s.iter().enumerate() {
        match b.get(i) {
            None => return true,
            Some(&x) if x != c => return x < c,
            _ => {}
        }
    }
    false
}

/// (number of new digits, cutoff): shifting left by `k` adds `delta` digits, minus one if the
/// original digits are less than `cutoff` (which is `5^k`).
static LEFT_CHEATS: [(usize, &str); 61] = [
    (0, ""),
    (1, "5"),
    (1, "25"),
    (1, "125"),
    (2, "625"),
    (2, "3125"),
    (2, "15625"),
    (3, "78125"),
    (3, "390625"),
    (3, "1953125"),
    (4, "9765625"),
    (4, "48828125"),
    (4, "244140625"),
    (4, "1220703125"),
    (5, "6103515625"),
    (5, "30517578125"),
    (5, "152587890625"),
    (6, "762939453125"),
    (6, "3814697265625"),
    (6, "19073486328125"),
    (7, "95367431640625"),
    (7, "476837158203125"),
    (7, "2384185791015625"),
    (7, "11920928955078125"),
    (8, "59604644775390625"),
    (8, "298023223876953125"),
    (8, "1490116119384765625"),
    (9, "7450580596923828125"),
    (9, "37252902984619140625"),
    (9, "186264514923095703125"),
    (10, "931322574615478515625"),
    (10, "4656612873077392578125"),
    (10, "23283064365386962890625"),
    (10, "116415321826934814453125"),
    (11, "582076609134674072265625"),
    (11, "2910383045673370361328125"),
    (11, "14551915228366851806640625"),
    (12, "72759576141834259033203125"),
    (12, "363797880709171295166015625"),
    (12, "1818989403545856475830078125"),
    (13, "9094947017729282379150390625"),
    (13, "45474735088646411895751953125"),
    (13, "227373675443232059478759765625"),
    (13, "1136868377216160297393798828125"),
    (14, "5684341886080801486968994140625"),
    (14, "28421709430404007434844970703125"),
    (14, "142108547152020037174224853515625"),
    (15, "710542735760100185871124267578125"),
    (15, "3552713678800500929355621337890625"),
    (15, "17763568394002504646778106689453125"),
    (16, "88817841970012523233890533447265625"),
    (16, "444089209850062616169452667236328125"),
    (16, "2220446049250313080847263336181640625"),
    (16, "11102230246251565404236316680908203125"),
    (17, "55511151231257827021181583404541015625"),
    (17, "277555756156289135105907917022705078125"),
    (17, "1387778780781445675529539585113525390625"),
    (18, "6938893903907228377647697925567626953125"),
    (18, "34694469519536141888238489627838134765625"),
    (18, "173472347597680709441192448139190673828125"),
    (19, "867361737988403547205962240695953369140625"),
];
