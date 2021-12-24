use super::{HummockResult, HummockValue, TableIterator};

mod concat;
pub use concat::*;
mod sorted;
pub use sorted::*;
mod user_key;
pub use user_key::*;

#[cfg(test)]
pub(crate) mod test_utils;
use async_trait::async_trait;

/// `HummockIterator` defines the interface of all iterators, including `TableIterator`,
/// `SortedIterator`, `UserKeyIterator` and `ConcatIterator`.
///
/// After create the iterator instance,
/// - if you want to iterate from the beginning, you need to then call its `rewind` method.
/// - if you want to iterate from some specific position, you need to then call its `seek` method.
///
/// Before calling
#[async_trait]
pub trait HummockIterator: Send + Sync {
    /// Move a valid iterator to the next key.
    ///
    /// Note:
    /// - Before calling this function, make sure the iterator `is_valid`.
    /// - After calling this function, you may first check whether the iterator `is_valid` again,
    ///   then get the new data by calling `key` and `value`.
    /// - If the position after calling this is invalid, this function WON'T return an `Err`. You
    ///   should check `is_valid` before continuing the iteration.
    ///
    /// # Panics
    /// This function will panic if the iterator is invalid.
    async fn next(&mut self) -> HummockResult<()>;

    /// Retrieve the current key.
    ///
    /// Note:
    /// - Before calling this function, make sure the iterator `is_valid`.
    /// - This function should be straightforward and return immediately.
    ///
    /// # Panics
    /// This function will panic if the iterator is invalid.
    fn key(&self) -> &[u8];

    /// Retrieve the current value, decoded as [`HummockValue`].
    ///
    /// Note:
    /// - Before calling this function, make sure the iterator `is_valid`.
    /// - This function should be straightforward and return immediately.
    ///
    /// # Panics
    /// This function will panic if the iterator is invalid, or the value cannot be decoded into
    /// [`HummockValue`].
    fn value(&self) -> HummockValue<&[u8]>;

    /// Indicate whether the iterator can be used.
    ///
    /// Note:
    /// - ONLY call `key`, `value`, and `next` if `is_valid` returns `true`.
    /// - This function should be straightforward and return immediately.
    fn is_valid(&self) -> bool;

    /// Reset the position of the iterator.
    ///
    /// Note:
    /// - Do not decide whether the position is valid or not by checking the returned error of this
    ///   function. This function WON'T return an `Err` if invalid. You should check `is_valid`
    ///   before starting iteration.
    async fn rewind(&mut self) -> HummockResult<()>;

    /// Reset iterator and seek to the first position where the key >= provided key.
    ///
    /// Note:
    /// - Do not decide whether the position is valid or not by checking the returned error of this
    ///   function. This function WON'T return an `Err` if invalid. You should check `is_valid`
    ///   before starting iteration.
    async fn seek(&mut self, key: &[u8]) -> HummockResult<()>;
}

pub enum HummockIteratorImpl {
    Table(TableIterator),
    Concat(Box<ConcatIterator>),
    Sorted(SortedIterator),

    #[cfg(test)]
    Test(test_utils::TestIterator),
}

pub type BoxedHummockIterator = Box<dyn HummockIterator>;
