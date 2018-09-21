#![deny(warnings, missing_docs, missing_debug_implementations)]
#![doc(html_root_url = "https://docs.rs/slotmap/0.2.1")]
#![crate_name = "slotmap"]
#![cfg_attr(feature = "unstable", feature(untagged_unions))]

//! # slotmap
//!
//! This library provides a container with persistent unique keys to access
//! stored values, [`SlotMap`]. Upon insertion a key is returned that can be
//! used to later access or remove the values. Insertion, removal and access all
//! take O(1) time with low overhead. Great for storing collections of objects
//! that need stable, safe references but have no clear ownership otherwise,
//! such as game entities or graph nodes.
//!
//! The difference between a [`BTreeMap`] or [`HashMap`] and a slot map is
//! that the slot map generates and returns the key when inserting a value. A
//! key is always unique and will only refer to the value that was inserted.
//! A slot map's main purpose is to simply own things in a safe and efficient
//! manner.
//!
//! # Examples
//!
//! ```
//! # use slotmap::*;
//! let mut sm = SlotMap::new();
//! let foo = sm.insert("foo");  // Key generated on insert.
//! let bar = sm.insert("bar");
//! assert_eq!(sm[foo], "foo");
//! assert_eq!(sm[bar], "bar");
//!
//! sm.remove(bar);
//! let reused = sm.insert("reuse");  // Space from bar reused.
//! assert_eq!(sm.contains_key(bar), false);  // After deletion a key stays invalid.
//! ```
//!
//! # Serialization through [`serde`]
//!
//! Both [`Key`] and the slot maps have full (de)seralization support through
//! the [`serde`] library. A key remains valid for a slot map even after one or
//! both have been serialized and deserialized! This makes storing or
//! transferring complicated referential structures and graphs a breeze. Care has
//! been taken such that deserializing keys and slot maps from untrusted sources
//! is safe.
//!
//! # Why not [`slab`]?
//!
//! Unlike [`slab`], the keys returned by [`SlotMap`] are versioned. This means
//! that once a key is removed, it stays removed, even if the physical storage
//! inside the slotmap is re-used for new elements. The [`Key`] is a
//! permanently unique<sup>*</sup> reference to the inserted value. Despite
//! supporting versioning, a [`SlotMap`] is not slower than [`slab`], by
//! internally using carefully checked unsafe code. A [`HopSlotMap`]
//! also provides faster iteration than [`slab`] does. Additionally, at the time
//! of writing [`slab`] does not support serialization.
//!
//! # Performance characteristics and implementation details
//!
//! Insertion, access and deletion is all O(1) with low overhead by storing the
//! elements inside a [`Vec`]. Unlike references or indices into a vector,
//! unless you remove a key it is never invalidated. Behind the scenes each
//! slot in the vector is a `(value, version)` tuple. After insertion the
//! returned key also contains a version. Only when the stored version and
//! version in a key match is a key valid. This allows us to reuse space in the
//! vector after deletion without letting removed keys point to spurious new
//! elements. <sup>*</sup>After 2<sup>31</sup> deletions and insertions to the
//! same underlying slot the version wraps around and such a spurious reference
//! could potentially occur. It is incredibly unlikely however, and in all
//! circumstances is the behavior safe. A slot map can hold up to
//! 2<sup>32</sup> - 2 elements at a time.
//!
//! The memory usage for each slot in [`SlotMap`] is `4 + max(sizeof(T), 4)`
//! rounded up to the alignment of `T`. Similarly it is `4 + max(sizeof(T), 12)`
//! for [`HopSlotMap`].
//!
//! # Choosing `SlotMap` or `HopSlotMap`
//!
//! A [`SlotMap`] can never shrink the size of its underlying storage, because for
//! each storage slot it must remember what the latest stored version was, even
//! if the slot is empty now. This means that iteration can be slow as it must
//! iterate over potentially a lot of empty slots.
//!
//! [`HopSlotMap`] solves this by maintaining more information on
//! insertion/removal, allowing it to iterate only over filled slots by 'hopping
//! over' contiguous blocks of vacant slots. This can give it significantly
//! better iteration speed.  If you expect to iterate over all elements in a
//! [`SlotMap`] a lot, choose [`HopSlotMap`]. The downside is that insertion and
//! removal is roughly twice as slow. Random access is the same speed for both.
//!
//! [`Vec`]: https://doc.rust-lang.org/std/vec/struct.Vec.html
//! [`BTreeMap`]: https://doc.rust-lang.org/std/collections/struct.BTreeMap.html
//! [`HashMap`]: https://doc.rust-lang.org/std/collections/struct.HashMap.html
//! [`Key`]: struct.Key.html
//! [`SlotMap`]: struct.SlotMap.html
//! [`HopSlotMap`]: hop/struct.HopSlotMap.html
//! [`serde`]: https://github.com/serde-rs/serde
//! [`slab`]: https://github.com/carllerche/slab

#[cfg(feature = "serde")]
#[macro_use]
extern crate serde;

#[cfg(test)]
#[macro_use]
extern crate quickcheck;

#[cfg(test)]
extern crate serde_json;

pub(crate) mod normal;
pub use normal::*;

pub mod hop;
pub use hop::HopSlotMap;

use std::num::NonZeroU32;

// Duplicated docs.

/// A trait for items that can go in a slot map. Due to current stable Rust
/// restrictions a type must be [`Copy`] to be placed in a slot map. If you must
/// store a type that is not [`Copy`] you must use nightly Rust and enable the
/// `unstable` feature for `slotmap` by editing your `Cargo.toml`.
///
/// ```norun
/// slotmap = { version = "...", features = ["unstable"] }
/// ```
///
/// This trait should already be automatically implemented for any type that is
/// slottable.
///
/// [`Copy`]: https://doc.rust-lang.org/std/marker/trait.Copy.html
#[cfg(not(feature = "unstable"))]
pub trait Slottable: Copy {}

/// A trait for items that can go in a slot map. Due to current stable Rust
/// restrictions a type must be [`Copy`] to be placed in a slot map. If you must
/// store a type that is not [`Copy`] you must use nightly Rust and enable the
/// `unstable` feature for `slotmap` by editing your `Cargo.toml`.
///
/// ```norun
/// slotmap = { version = "...", features = ["unstable"] }
/// ```
///
/// This trait should already be automatically implemented for any type that is
/// slottable.
///
/// [`Copy`]: https://doc.rust-lang.org/std/marker/trait.Copy.html
#[cfg(feature = "unstable")]
pub trait Slottable {}

#[cfg(not(feature = "unstable"))]
impl<T: Copy> Slottable for T {}

#[cfg(feature = "unstable")]
impl<T> Slottable for T {}



/// Key used to access stored values in a slot map.
///
/// Do not use a key from one slot map in another. The behavior is safe but
/// non-sensical (and might panic in case of out-of-bounds). Keys implement
/// `Ord` so they can be used in e.g.
/// [`BTreeMap`](https://doc.rust-lang.org/std/collections/struct.BTreeMap.html)
/// but their order is arbitrary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Key {
    idx: u32,
    version: NonZeroU32,
}

impl Key {
    fn new(idx: u32, version: u32) -> Self {
        Self {
            idx,
            version: NonZeroU32::new(version).unwrap(),
        }
    }

    /// Creates a new key that is always invalid and distinct from any non-null
    /// key. A null key can only be created through this method, or default
    /// initialization of `Key`.
    ///
    /// A null key is always invalid, but an invalid key (that is, a key that
    /// has been removed from the slot map) does not become a null key. A null
    /// is safe to use with any safe method of any slot map instance.
    ///
    /// # Examples
    ///
    /// ```
    /// # use slotmap::*;
    /// let mut sm = SlotMap::<i32>::new();
    /// let nk = Key::null();
    /// assert!(nk.is_null());
    /// assert_eq!(sm.get(nk), None);
    /// ```
    pub fn null() -> Self {
        Self::new(std::u32::MAX, 1)
    }

    /// Checks if a key is null. There is only a single null key, that is
    /// `a.is_null() && b.is_null()` implies `a == b`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use slotmap::*;
    /// let a = Key::null();
    /// let b = Key::default();
    /// assert_eq!(a, b);
    /// ```
    pub fn is_null(self) -> bool {
        self.idx == std::u32::MAX
    }
}

impl Default for Key {
    fn default() -> Self {
        Self::null()
    }
}

// Serialization with serde.
#[cfg(feature = "serde")]
mod serialize {
    use super::*;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    pub struct SerKey {
        idx: u32,
        version: u32,
    }

    impl Serialize for Key {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            let ser_key = SerKey {
                idx: self.idx,
                version: self.version.get(),
            };
            ser_key.serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for Key {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
        {
            let mut ser_key: SerKey = Deserialize::deserialize(deserializer)?;

            // Ensure a.is_null() && b.is_null() implies a == b.
            if ser_key.idx == std::u32::MAX {
                ser_key.version = 1;
            }

            ser_key.version |= 1; // Ensure version is odd.
            Ok(Key::new(ser_key.idx, ser_key.version))
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "serde")]
    use super::*;

    #[cfg(feature = "serde")]
    #[test]
    fn key_serde() {
        // Check round-trip through serde.
        let mut sm = SlotMap::new();
        let k = sm.insert(42);
        let ser = serde_json::to_string(&k).unwrap();
        let de: Key = serde_json::from_str(&ser).unwrap();
        assert_eq!(k, de);

        // Even if a malicious entity sends up even (unoccupied) versions in the
        // key, we make the version point to the occupied version.
        let malicious = serde_json::from_str::<Key>(&r#"{"idx":0,"version":4}"#).unwrap();
        assert_eq!(malicious.version.get(), 5);
    }
}
