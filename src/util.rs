use std::collections::BTreeSet;

pub struct SkippingIter<'a> {
    curr: i32,
    inner: &'a BTreeSet<i32>,
}

impl<'a> SkippingIter<'a> {
    pub fn new(inner: &'a BTreeSet<i32>) -> Self {
        Self { curr: 1, inner }
    }
}

impl<'a> Iterator for SkippingIter<'a> {
    type Item = i32;

    fn next(&mut self) -> Option<Self::Item> {
        if self.inner.contains(&self.curr) {
            self.curr += 1;
            self.next()
        } else {
            let ret = self.curr;
            self.curr += 1;
            Some(ret)
        }
    }
}

#[test]
fn test_skipping_iter() {
    let existing = BTreeSet::from([2, 4, 5, 7]);
    let mut iter = SkippingIter::new(&existing);

    assert_eq!(iter.next(), Some(1));
    assert_eq!(iter.next(), Some(3));
    assert_eq!(iter.next(), Some(6));
    assert_eq!(iter.next(), Some(8));
    assert_eq!(iter.next(), Some(9));
    assert_eq!(iter.next(), Some(10));
}
