//! HTTP Range resolution and byte coverage tracking (pure code).

/// Maximum number of disjoint ranges tracked by [`Coverage`].
const MAX_RANGES: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangeError {
    Unsatisfiable,
    Multi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resolved {
    pub start: u64,
    /// Inclusive.
    pub end: u64,
    pub partial: bool,
}

/// Resolve an optional `Range` header against a file of `size` bytes.
///
/// A missing or syntactically invalid header (RFC 9110: ignore) yields the whole file,
/// untruncated, with `partial = false`. A valid single range is clamped to the file and
/// truncated to `max_len` bytes. Never panics: all arithmetic is checked or saturating.
pub fn resolve(header: Option<&str>, size: u64, max_len: u64) -> Result<Resolved, RangeError> {
    let whole = Resolved {
        start: 0,
        end: size.saturating_sub(1),
        partial: false,
    };
    let Some(h) = header else { return Ok(whole) };
    let h = h.trim();
    let Some((unit, spec)) = h.split_once('=') else {
        return Ok(whole);
    };
    if !unit.trim().eq_ignore_ascii_case("bytes") {
        return Ok(whole);
    }
    if spec.contains(',') {
        return Err(RangeError::Multi);
    }
    let Some((a, b)) = spec.trim().split_once('-') else {
        return Ok(whole);
    };
    let (a, b) = (a.trim(), b.trim());
    let parse = |s: &str| s.parse::<u64>().ok();

    let (start, end) = match (a.is_empty(), b.is_empty()) {
        (true, true) => return Ok(whole),
        (true, false) => {
            // Suffix: last N bytes.
            let Some(n) = parse(b) else { return Ok(whole) };
            if n == 0 || size == 0 {
                return Err(RangeError::Unsatisfiable);
            }
            (size.saturating_sub(n), size - 1)
        }
        (false, true) => {
            let Some(s) = parse(a) else { return Ok(whole) };
            (s, size.saturating_sub(1))
        }
        (false, false) => {
            let (Some(s), Some(e)) = (parse(a), parse(b)) else {
                return Ok(whole);
            };
            if e < s {
                return Err(RangeError::Unsatisfiable);
            }
            (s, e.min(size.saturating_sub(1)))
        }
    };
    if start >= size {
        return Err(RangeError::Unsatisfiable);
    }
    let cap_end = start.saturating_add(max_len.max(1) - 1);
    Ok(Resolved {
        start,
        end: end.min(cap_end),
        partial: true,
    })
}

/// Set of covered byte ranges, kept as sorted, merged, half-open `(start, end)` pairs.
#[derive(Debug, Default)]
pub struct Coverage {
    ranges: Vec<(u64, u64)>,
}

impl Coverage {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add `[start, end_excl)`. Returns false when ignored because [`MAX_RANGES`] disjoint
    /// ranges already exist and the new range does not merge with any of them.
    pub fn add(&mut self, start: u64, end_excl: u64) -> bool {
        if end_excl <= start {
            return true;
        }
        // Ranges touching or overlapping [start, end_excl]: sorted and disjoint, so contiguous.
        let lo = self.ranges.partition_point(|&(_, e)| e < start);
        let hi = self.ranges.partition_point(|&(s, _)| s <= end_excl);
        if lo >= hi {
            if self.ranges.len() >= MAX_RANGES {
                return false;
            }
            self.ranges.insert(lo, (start, end_excl));
            return true;
        }
        let ns = start.min(self.ranges[lo].0);
        let ne = end_excl.max(self.ranges[hi - 1].1);
        self.ranges.splice(lo..hi, [(ns, ne)]);
        true
    }

    pub fn covered(&self) -> u64 {
        self.ranges.iter().map(|&(s, e)| e - s).sum()
    }

    pub fn is_complete(&self, size: u64) -> bool {
        size == 0 || (self.ranges.len() == 1 && self.ranges[0].0 == 0 && self.ranges[0].1 >= size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX: u64 = 100;

    fn r(h: &str, size: u64) -> Result<(u64, u64, bool), RangeError> {
        resolve(Some(h), size, MAX).map(|x| (x.start, x.end, x.partial))
    }

    #[test]
    fn no_header_is_the_whole_file_untruncated() {
        let x = resolve(None, 1_000_000, MAX).unwrap();
        assert_eq!((x.start, x.end, x.partial), (0, 999_999, false));
    }

    #[test]
    fn closed_open_and_suffix_ranges() {
        assert_eq!(r("bytes=10-19", 1000).unwrap(), (10, 19, true));
        assert_eq!(r("bytes=990-", 1000).unwrap(), (990, 999, true));
        assert_eq!(r("bytes=-5", 1000).unwrap(), (995, 999, true));
        assert_eq!(r("bytes=900-5000", 1000).unwrap(), (900, 999, true)); // end clamped
    }

    #[test]
    fn long_ranges_are_truncated_to_the_fragment_cap() {
        assert_eq!(r("bytes=0-", 1000).unwrap(), (0, 99, true));
        assert_eq!(r("bytes=50-900", 1000).unwrap(), (50, 149, true));
        assert_eq!(r("bytes=-500", 1000).unwrap(), (500, 599, true));
    }

    #[test]
    fn hostile_ranges() {
        assert!(matches!(
            r("bytes=5-1", 1000),
            Err(RangeError::Unsatisfiable)
        ));
        assert!(matches!(
            r("bytes=1000-", 1000),
            Err(RangeError::Unsatisfiable)
        ));
        assert!(matches!(
            r("bytes=-0", 1000),
            Err(RangeError::Unsatisfiable)
        ));
        assert!(matches!(r("bytes=0-1,5-6", 1000), Err(RangeError::Multi)));
        assert!(matches!(r("bytes=0-1", 0), Err(RangeError::Unsatisfiable)));
        // Syntactically invalid or another unit: ignored, whole file.
        for bad in [
            "items=0-5",
            "bytes=abc",
            "bytes=",
            "garbage",
            "bytes=-",
            "bytes=1-x",
        ] {
            let x = resolve(Some(bad), 1000, MAX).unwrap();
            assert!(!x.partial, "{bad}");
            assert_eq!((x.start, x.end), (0, 999), "{bad}");
        }
    }

    #[test]
    fn u64_overflow_is_invalid_and_ignored_without_panic() {
        // Does not fit in u64: parse fails, so the header is invalid and ignored (whole file).
        for bad in [
            "bytes=99999999999999999999-",
            "bytes=0-99999999999999999999",
            "bytes=-99999999999999999999",
        ] {
            let x = resolve(Some(bad), 10, MAX).unwrap();
            assert!(!x.partial, "{bad}");
            assert_eq!((x.start, x.end), (0, 9), "{bad}");
        }
        // Huge but valid u64 values: no overflow in start + max_len.
        let x = resolve(Some("bytes=0-18446744073709551615"), u64::MAX, u64::MAX).unwrap();
        assert_eq!((x.start, x.end, x.partial), (0, u64::MAX - 1, true));
        let x = resolve(Some("bytes=18446744073709551000-"), u64::MAX, u64::MAX).unwrap();
        assert_eq!(x.end, u64::MAX - 1);
        assert!(matches!(
            r("bytes=18446744073709551615-", 10),
            Err(RangeError::Unsatisfiable)
        ));
    }

    #[test]
    fn coverage_merges_and_counts_unique_bytes() {
        let mut c = Coverage::new();
        assert!(c.add(0, 10));
        assert!(c.add(5, 20)); // overlap
        assert!(c.add(20, 30)); // adjacent
        assert_eq!(c.covered(), 30);
        assert!(c.add(40, 50));
        assert_eq!(c.covered(), 40);
        assert!(!c.is_complete(50));
        c.add(30, 40);
        assert!(c.is_complete(50));
    }

    #[test]
    fn coverage_ignores_empty_and_is_bounded() {
        let mut c = Coverage::new();
        assert!(c.add(5, 5));
        assert_eq!(c.covered(), 0);
        for i in 0..600u64 {
            c.add(i * 10, i * 10 + 1); // 600 disjoint ranges
        }
        assert_eq!(c.covered(), 512); // beyond 512 disjoint ranges new ones are ignored
    }
}
