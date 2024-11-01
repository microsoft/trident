//! Helper structs for partition details

/// A list of partition attributes
pub(crate) struct PartitionAttributeList<'a, T>(pub(crate) Vec<PartitionAttribute<'a, T>>);

/// A generic partition attribute
///
/// Can hold partition type, size, etc.
pub(crate) struct PartitionAttribute<'a, T> {
    pub id: &'a str,
    pub value: T,
}

impl<'a, T> PartitionAttributeList<'a, T> {
    pub(crate) fn new(id: &'a str, value: T) -> Self {
        Self(vec![PartitionAttribute { id, value }])
    }
    /// Returns whether the list of partition details is empty
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &PartitionAttribute<'a, T>> {
        self.0.iter()
    }
}

impl<'a, T> PartitionAttributeList<'a, T>
where
    T: PartialEq,
{
    /// Returns whether the list of partition detail values is homogeneous
    /// i.e. all values are the same
    pub(crate) fn is_homogeneous(&self) -> bool {
        if self.is_empty() {
            return true;
        }

        let first = self.0.first().map(|pd| &pd.value);
        self.0.iter().all(|pd| &pd.value == first.unwrap())
    }

    /// When the list is homogeneous, returns the common value
    /// Otherwise, returns None
    pub(crate) fn get_homogeneous(&self) -> Option<&T> {
        if self.is_homogeneous() && !self.is_empty() {
            self.0.first().map(|pd| &pd.value)
        } else {
            None
        }
    }
}

impl<'a, T> IntoIterator for PartitionAttributeList<'a, T> {
    type Item = PartitionAttribute<'a, T>;
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a, T> FromIterator<PartitionAttribute<'a, T>> for PartitionAttributeList<'a, T> {
    fn from_iter<I: IntoIterator<Item = PartitionAttribute<'a, T>>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_partition_attribute_list() {
        // Empty list
        let list = PartitionAttributeList::<i32>(vec![]);
        assert!(list.is_empty());
        assert!(list.is_homogeneous());
        assert_eq!(list.get_homogeneous(), None);

        // 1 item list
        let list = PartitionAttributeList::new("id", 1);
        assert!(!list.is_empty());
        assert!(list.is_homogeneous());
        assert_eq!(list.get_homogeneous(), Some(&1));

        // 2 item list
        let list = PartitionAttributeList(vec![
            PartitionAttribute { id: "id", value: 1 },
            PartitionAttribute { id: "id", value: 1 },
        ]);
        assert!(!list.is_empty());
        assert!(list.is_homogeneous());
        assert_eq!(list.get_homogeneous(), Some(&1));

        // Homogeneous N item list
        let list = PartitionAttributeList(
            (1..=10)
                .map(|_| PartitionAttribute {
                    id: "id",
                    value: 42,
                })
                .collect(),
        );
        assert!(!list.is_empty());
        assert!(list.is_homogeneous());
        assert_eq!(list.get_homogeneous(), Some(&42));

        // Heterogeneous 2 item list with different values
        let list = PartitionAttributeList(vec![
            PartitionAttribute { id: "id", value: 1 },
            PartitionAttribute { id: "id", value: 2 },
        ]);
        assert!(!list.is_empty());

        // Heterogeneous N item list
        let list = PartitionAttributeList(
            (1..=10)
                .map(|i| PartitionAttribute { id: "id", value: i })
                .collect(),
        );
        assert!(!list.is_empty());
        assert!(!list.is_homogeneous());
        assert_eq!(list.get_homogeneous(), None);
    }
}
