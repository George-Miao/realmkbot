use std::collections::BTreeSet;

use grammers_client::InvocationError;

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

pub async fn invoke<F, R>(mut f: F) -> Result<R, InvocationError>
where
    F: AsyncFnMut() -> Result<R, InvocationError>,
{
    loop {
        let res = f().await;

        match res {
            Err(InvocationError::Rpc(rpc)) if rpc.code == 420 => {
                let wait_time = rpc.value.unwrap_or(30);
                info!("Flood wait, waiting for {wait_time} seconds");
                tokio::time::sleep(std::time::Duration::from_secs(wait_time as _)).await;
            }
            Err(e) => return Err(e),
            Ok(res) => return Ok(res),
        }
    }
}
