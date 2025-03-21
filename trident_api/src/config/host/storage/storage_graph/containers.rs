use std::{fmt::Display, ops::Deref, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::BlockDeviceId;

/// A list to only allow or block certain types of objects in rules.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum AllowBlockList<T> {
    None,
    Any,
    Allow(Vec<T>),
    Block(Vec<T>),
}

impl<T: PartialEq> AllowBlockList<T> {
    /// Creates a new list that blocks all items except the ones in the list.
    pub(crate) fn new_allow(items: impl IntoIterator<Item = impl Into<T>>) -> Self {
        AllowBlockList::Allow(items.into_iter().map(Into::into).collect())
    }

    /// Returns whether the list contains the given item.
    pub(crate) fn contains(&self, item: impl PartialEq<T>) -> bool {
        match self {
            AllowBlockList::None => false,
            AllowBlockList::Any => true,
            AllowBlockList::Allow(items) => items.iter().any(|t| item == *t),
            AllowBlockList::Block(items) => items.iter().all(|t| item != *t),
        }
    }
}

/// Wrapper for a list of FileSystemSourceKind
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ItemList<T>(pub Vec<T>)
where
    T: Copy + Clone + PartialEq + Eq + Display;

impl<T> ItemList<T>
where
    T: Copy + Clone + PartialEq + Eq + Display,
{
    /// Returns whether the list contains the given item.
    pub(crate) fn contains(&self, fs_src_kind: T) -> bool {
        self.0.contains(&fs_src_kind)
    }

    /// Returns a new ItemList with the given filter applied.
    pub fn filter(&self, f: impl Fn(&T) -> bool) -> Self {
        Self(self.0.iter().filter(|kind| f(kind)).cloned().collect())
    }
}

/// Specialization of AllowBlockList for paths because PathBuf is weird about
/// Display.
pub struct PathAllowBlockList(AllowBlockList<PathBuf>);

impl From<AllowBlockList<PathBuf>> for PathAllowBlockList {
    fn from(list: AllowBlockList<PathBuf>) -> Self {
        PathAllowBlockList(list)
    }
}

impl Deref for PathAllowBlockList {
    type Target = AllowBlockList<PathBuf>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// A list of generic block device attributes.
pub(crate) struct BlkDevAttrList<'a, T>(pub(crate) Vec<BlkDevAttr<'a, T>>);

/// A generic block device attribute.
///
/// Can hold partition type, size, etc.
pub(crate) struct BlkDevAttr<'a, T> {
    pub id: &'a BlockDeviceId,
    pub value: T,
}

impl<T> Default for BlkDevAttrList<'_, T> {
    fn default() -> Self {
        Self(vec![])
    }
}

impl<'a, T> BlkDevAttrList<'a, T> {
    /// Creates a new list with a single attribute.
    pub(crate) fn new(id: &'a BlockDeviceId, value: T) -> Self {
        Self(vec![BlkDevAttr { id, value }])
    }

    /// Returns whether the list of attributes details is empty.
    #[allow(dead_code)]
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Returns an iterator over the attributes.
    pub(crate) fn iter(&self) -> impl Iterator<Item = &BlkDevAttr<'a, T>> {
        self.0.iter()
    }
}

impl<'a, T> Extend<BlkDevAttr<'a, T>> for BlkDevAttrList<'a, T> {
    fn extend<I: IntoIterator<Item = BlkDevAttr<'a, T>>>(&mut self, iter: I) {
        self.0.extend(iter);
    }
}

impl<T> BlkDevAttrList<'_, T>
where
    T: PartialEq,
{
    /// Returns whether the list of attributes is homogeneous
    /// i.e. all values are the same.
    pub(crate) fn is_homogeneous(&self) -> bool {
        let Some(first) = self.0.first() else {
            // An empty list is homogeneous
            return true;
        };
        self.0.iter().all(|pd| pd.value == first.value)
    }

    /// When the list is homogeneous, returns the common value. Otherwise,
    /// returns None.
    pub(crate) fn get_homogeneous(&self) -> Option<&T> {
        self.is_homogeneous()
            .then(|| self.0.first().map(|pd| &pd.value))
            .flatten()
    }
}

impl<'a, T> IntoIterator for BlkDevAttrList<'a, T> {
    type Item = BlkDevAttr<'a, T>;
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a, T> FromIterator<BlkDevAttr<'a, T>> for BlkDevAttrList<'a, T> {
    fn from_iter<I: IntoIterator<Item = BlkDevAttr<'a, T>>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_partition_attribute_list() {
        // Empty list
        let list = BlkDevAttrList::<i32>(vec![]);
        assert!(list.is_empty());
        assert!(list.is_homogeneous());
        assert_eq!(list.get_homogeneous(), None);

        let id: BlockDeviceId = "id".into();

        // 1 item list
        let list = BlkDevAttrList::new(&id, 1);
        assert!(!list.is_empty());
        assert!(list.is_homogeneous());
        assert_eq!(list.get_homogeneous(), Some(&1));

        // 2 item list
        let list = BlkDevAttrList(vec![
            BlkDevAttr { id: &id, value: 1 },
            BlkDevAttr { id: &id, value: 1 },
        ]);
        assert!(!list.is_empty());
        assert!(list.is_homogeneous());
        assert_eq!(list.get_homogeneous(), Some(&1));

        // Homogeneous N item list
        let list = BlkDevAttrList(
            (1..=10)
                .map(|_| BlkDevAttr { id: &id, value: 42 })
                .collect(),
        );
        assert!(!list.is_empty());
        assert!(list.is_homogeneous());
        assert_eq!(list.get_homogeneous(), Some(&42));

        // Heterogeneous 2 item list with different values
        let list = BlkDevAttrList(vec![
            BlkDevAttr { id: &id, value: 1 },
            BlkDevAttr { id: &id, value: 2 },
        ]);
        assert!(!list.is_empty());

        // Heterogeneous N item list
        let list = BlkDevAttrList((1..=10).map(|i| BlkDevAttr { id: &id, value: i }).collect());
        assert!(!list.is_empty());
        assert!(!list.is_homogeneous());
        assert_eq!(list.get_homogeneous(), None);
    }
}
