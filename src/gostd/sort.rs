//! Follows the behaviour of Go's standard library sort package (The Go Authors, BSD-style licence).
//!
//! Go's `sort.Sort` is an unstable pattern-defeating quicksort. The order it leaves equal elements in, and
//! the order in which it compares elements (which decides the first comparison that fails), are observable,
//! so this is a direct port. The comparison may fail; the first failure aborts the sort like Go's panic.

/// A sequence sorted in place.
pub trait Data {
    type Error;
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn less(&mut self, i: usize, j: usize) -> Result<bool, Self::Error>;
    fn swap(&mut self, i: usize, j: usize);
}

/// Go's `sort.Sort`.
pub fn sort<D: Data>(data: &mut D) -> Result<(), D::Error> {
    let n = data.len();
    if n <= 1 {
        return Ok(());
    }
    let limit = bits_len(n);
    pdqsort(data, 0, n, limit)
}

/// `bits.Len`: the number of bits needed to represent `x`.
fn bits_len(x: usize) -> usize {
    (usize::BITS - x.leading_zeros()) as usize
}

fn insertion_sort<D: Data>(data: &mut D, a: usize, b: usize) -> Result<(), D::Error> {
    for i in a + 1..b {
        let mut j = i;
        while j > a && data.less(j, j - 1)? {
            data.swap(j, j - 1);
            j -= 1;
        }
    }
    Ok(())
}

fn sift_down<D: Data>(data: &mut D, lo: usize, hi: usize, first: usize) -> Result<(), D::Error> {
    let mut root = lo;
    loop {
        let mut child = 2 * root + 1;
        if child >= hi {
            return Ok(());
        }
        if child + 1 < hi && data.less(first + child, first + child + 1)? {
            child += 1;
        }
        if !data.less(first + root, first + child)? {
            return Ok(());
        }
        data.swap(first + root, first + child);
        root = child;
    }
}

fn heap_sort<D: Data>(data: &mut D, a: usize, b: usize) -> Result<(), D::Error> {
    let first = a;
    let hi = b - a;
    for i in (0..=(hi - 1) / 2).rev() {
        sift_down(data, i, hi, first)?;
    }
    for i in (0..hi).rev() {
        data.swap(first, first + i);
        sift_down(data, 0, i, first)?;
    }
    Ok(())
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Hint {
    Unknown,
    Increasing,
    Decreasing,
}

fn pdqsort<D: Data>(data: &mut D, mut a: usize, mut b: usize, mut limit: usize) -> Result<(), D::Error> {
    const MAX_INSERTION: usize = 12;
    let mut was_balanced = true;
    let mut was_partitioned = true;
    loop {
        let length = b - a;
        if length <= MAX_INSERTION {
            return insertion_sort(data, a, b);
        }
        if limit == 0 {
            return heap_sort(data, a, b);
        }
        if !was_balanced {
            break_patterns(data, a, b);
            limit -= 1;
        }

        let (mut pivot, mut hint) = choose_pivot(data, a, b)?;
        if hint == Hint::Decreasing {
            reverse_range(data, a, b);
            pivot = (b - 1) - (pivot - a);
            hint = Hint::Increasing;
        }

        if was_balanced && was_partitioned && hint == Hint::Increasing && partial_insertion_sort(data, a, b)? {
            return Ok(());
        }

        if a > 0 && !data.less(a - 1, pivot)? {
            a = partition_equal(data, a, b, pivot)?;
            continue;
        }

        let (mid, already_partitioned) = partition(data, a, b, pivot)?;
        was_partitioned = already_partitioned;

        let (left_len, right_len) = (mid - a, b - mid);
        let balance_threshold = length / 8;
        if left_len < right_len {
            was_balanced = left_len >= balance_threshold;
            pdqsort(data, a, mid, limit)?;
            a = mid + 1;
        } else {
            was_balanced = right_len >= balance_threshold;
            pdqsort(data, mid + 1, b, limit)?;
            b = mid;
        }
    }
}

/// Indices may run one below `a`, like Go's signed ints; `i64` keeps that arithmetic exact.
fn partition<D: Data>(data: &mut D, a: usize, b: usize, pivot: usize) -> Result<(usize, bool), D::Error> {
    data.swap(a, pivot);
    let (mut i, mut j) = (a as i64 + 1, b as i64 - 1);
    while i <= j && data.less(i as usize, a)? {
        i += 1;
    }
    while i <= j && !data.less(j as usize, a)? {
        j -= 1;
    }
    if i > j {
        data.swap(j as usize, a);
        return Ok((j as usize, true));
    }
    data.swap(i as usize, j as usize);
    i += 1;
    j -= 1;
    loop {
        while i <= j && data.less(i as usize, a)? {
            i += 1;
        }
        while i <= j && !data.less(j as usize, a)? {
            j -= 1;
        }
        if i > j {
            break;
        }
        data.swap(i as usize, j as usize);
        i += 1;
        j -= 1;
    }
    data.swap(j as usize, a);
    Ok((j as usize, false))
}

fn partition_equal<D: Data>(data: &mut D, a: usize, b: usize, pivot: usize) -> Result<usize, D::Error> {
    data.swap(a, pivot);
    let (mut i, mut j) = (a as i64 + 1, b as i64 - 1);
    loop {
        while i <= j && !data.less(a, i as usize)? {
            i += 1;
        }
        while i <= j && data.less(a, j as usize)? {
            j -= 1;
        }
        if i > j {
            break;
        }
        data.swap(i as usize, j as usize);
        i += 1;
        j -= 1;
    }
    Ok(i as usize)
}

fn partial_insertion_sort<D: Data>(data: &mut D, a: usize, b: usize) -> Result<bool, D::Error> {
    const MAX_STEPS: usize = 5;
    const SHORTEST_SHIFTING: usize = 50;
    let mut i = a + 1;
    for _ in 0..MAX_STEPS {
        while i < b && !data.less(i, i - 1)? {
            i += 1;
        }
        if i == b {
            return Ok(true);
        }
        if b - a < SHORTEST_SHIFTING {
            return Ok(false);
        }
        data.swap(i, i - 1);
        if i - a >= 2 {
            let mut j = i - 1;
            while j >= 1 {
                if !data.less(j, j - 1)? {
                    break;
                }
                data.swap(j, j - 1);
                j -= 1;
            }
        }
        if b - i >= 2 {
            let mut j = i + 1;
            while j < b {
                if !data.less(j, j - 1)? {
                    break;
                }
                data.swap(j, j - 1);
                j += 1;
            }
        }
    }
    Ok(false)
}

fn break_patterns<D: Data>(data: &mut D, a: usize, b: usize) {
    let length = b - a;
    if length >= 8 {
        let mut random = length as u64;
        let modulus = 1usize << bits_len(length);
        let base = a + (length / 4) * 2;
        for idx in base - 1..=base + 1 {
            random ^= random << 13;
            random ^= random >> 7;
            random ^= random << 17;
            let mut other = (random as usize) & (modulus - 1);
            if other >= length {
                other -= length;
            }
            data.swap(idx, a + other);
        }
    }
}

fn choose_pivot<D: Data>(data: &mut D, a: usize, b: usize) -> Result<(usize, Hint), D::Error> {
    const SHORTEST_NINTHER: usize = 50;
    const MAX_SWAPS: usize = 4 * 3;
    let l = b - a;
    let mut swaps = 0;
    let mut i = a + l / 4;
    let mut j = a + l / 4 * 2;
    let mut k = a + l / 4 * 3;
    if l >= 8 {
        if l >= SHORTEST_NINTHER {
            i = median(data, i - 1, i, i + 1, &mut swaps)?;
            j = median(data, j - 1, j, j + 1, &mut swaps)?;
            k = median(data, k - 1, k, k + 1, &mut swaps)?;
        }
        j = median(data, i, j, k, &mut swaps)?;
    }
    Ok(match swaps {
        0 => (j, Hint::Increasing),
        MAX_SWAPS => (j, Hint::Decreasing),
        _ => (j, Hint::Unknown),
    })
}

fn order2<D: Data>(data: &mut D, a: usize, b: usize, swaps: &mut usize) -> Result<(usize, usize), D::Error> {
    if data.less(b, a)? {
        *swaps += 1;
        return Ok((b, a));
    }
    Ok((a, b))
}

fn median<D: Data>(data: &mut D, a: usize, b: usize, c: usize, swaps: &mut usize) -> Result<usize, D::Error> {
    let (a, b) = order2(data, a, b, swaps)?;
    let (b, c) = order2(data, b, c, swaps)?;
    let (_, b) = order2(data, a, b, swaps)?;
    let _ = c;
    Ok(b)
}

fn reverse_range<D: Data>(data: &mut D, a: usize, b: usize) {
    let (mut i, mut j) = (a, b - 1);
    while i < j {
        data.swap(i, j);
        i += 1;
        j -= 1;
    }
}

/// Sorting `float64` values the way Go's `sort.Float64s` (`slices.Sort`) does: NaNs first.
pub fn sort_floats(values: &mut [f64]) {
    struct Floats<'a>(&'a mut [f64]);
    impl Data for Floats<'_> {
        type Error = std::convert::Infallible;
        fn len(&self) -> usize {
            self.0.len()
        }
        fn less(&mut self, i: usize, j: usize) -> Result<bool, Self::Error> {
            let (x, y) = (self.0[i], self.0[j]);
            Ok(x < y || x.is_nan() && !y.is_nan())
        }
        fn swap(&mut self, i: usize, j: usize) {
            self.0.swap(i, j);
        }
    }
    let Ok(()) = sort(&mut Floats(values));
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Pairs(Vec<(i32, usize)>);

    impl Data for Pairs {
        type Error = ();
        fn len(&self) -> usize {
            self.0.len()
        }
        fn less(&mut self, i: usize, j: usize) -> Result<bool, ()> {
            Ok(self.0[i].0 < self.0[j].0)
        }
        fn swap(&mut self, i: usize, j: usize) {
            self.0.swap(i, j);
        }
    }

    #[test]
    fn sorts_large_inputs() {
        let mut data = Pairs((0..500).map(|i| (((i * 7919) % 101) as i32, i)).collect());
        sort(&mut data).unwrap();
        assert!(data.0.windows(2).all(|w| w[0].0 <= w[1].0));
    }

    #[test]
    fn sorts_floats_with_nan_first() {
        let mut v = vec![3.0, f64::NAN, -1.0, 2.0];
        sort_floats(&mut v);
        assert!(v[0].is_nan());
        assert_eq!(&v[1..], &[-1.0, 2.0, 3.0]);
    }
}
