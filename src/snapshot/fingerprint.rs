//! A fast in-process content hash of anything `Serialize`. It walks the same
//! serde shape that JSON would, hashing primitives' bits, string bytes,
//! lengths, field names and variant tags. It never formats text, so a large
//! description (resolved SFZ regions run to hundreds of thousands of fields)
//! hashes several times faster than its JSON. [`Fingerprint`] folds the walk
//! into 64 bits: not a security hash, only a stale-description guard.
//! [`Digest`] feeds the same unambiguous encoding to SHA-256, for in-process
//! content keys that must not collide. Neither is stable across builds.
use serde::ser::{self, Serialize};
use std::fmt;

const K0: u64 = 0x9E37_79B9_7F4A_7C15;
const K1: u64 = 0xA076_1D64_78BD_642F;

#[inline(always)]
fn fold(a: u64, b: u64) -> u64 {
    let product = (a as u128).wrapping_mul(b as u128);
    (product as u64) ^ ((product >> 64) as u64)
}

/// What the walk produces: tags, primitives and length-prefixed bytes.
pub(crate) trait Sink {
    fn word(&mut self, w: u64);
    fn bytes(&mut self, bytes: &[u8]);
}

pub(crate) struct Fold { state: u64, count: u64 }
impl Sink for Fold {
    #[inline(always)]
    fn word(&mut self, w: u64) {
        self.count = self.count.wrapping_add(1);
        self.state = fold(self.state ^ w, K1 ^ self.count.wrapping_mul(K0));
    }
    fn bytes(&mut self, bytes: &[u8]) {
        self.word(bytes.len() as u64);
        let (chunks, rest) = bytes.as_chunks::<8>();
        for &chunk in chunks {
            self.word(u64::from_le_bytes(chunk));
        }
        if !rest.is_empty() {
            let mut last = [0u8; 8];
            last[..rest.len()].copy_from_slice(rest);
            self.word(u64::from_le_bytes(last));
        }
    }
}

/// SHA-256 of the walk, buffered so words do not each pay an update call.
pub(crate) struct Sha { hasher: sha2::Sha256, buffer: Vec<u8> }
impl Sha {
    const CAPACITY: usize = 1 << 14;
    fn flush(&mut self) {
        use sha2::Digest as _;
        self.hasher.update(&self.buffer);
        self.buffer.clear();
    }
}
impl Sink for Sha {
    #[inline(always)]
    fn word(&mut self, w: u64) {
        if self.buffer.len() + 8 > Self::CAPACITY { self.flush(); }
        self.buffer.extend_from_slice(&w.to_le_bytes());
    }
    fn bytes(&mut self, bytes: &[u8]) {
        self.word(bytes.len() as u64);
        if self.buffer.len() + bytes.len() > Self::CAPACITY {
            self.flush();
            if bytes.len() > Self::CAPACITY {
                use sha2::Digest as _;
                self.hasher.update(bytes);
                return;
            }
        }
        self.buffer.extend_from_slice(bytes);
    }
}

/// The serde walk over a [`Sink`].
pub(crate) struct Walk<S>(S);
pub(crate) type Fingerprint = Walk<Fold>;
pub(crate) type Digest = Walk<Sha>;

impl<S: Sink> Walk<S> {
    #[inline(always)]
    fn word(&mut self, w: u64) { self.0.word(w) }
    fn bytes(&mut self, bytes: &[u8]) { self.0.bytes(bytes) }
    pub(crate) fn add<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        value.serialize(&mut *self)
    }
}
impl Fingerprint {
    pub(crate) fn new() -> Self { Walk(Fold { state: K0, count: 0 }) }
    pub(crate) fn finish(&self) -> u64 { fold(self.0.state ^ self.0.count, K0) }
}
impl Digest {
    pub(crate) fn new() -> Self {
        use sha2::Digest as _;
        Walk(Sha { hasher: sha2::Sha256::new(), buffer: Vec::with_capacity(Sha::CAPACITY) })
    }
    pub(crate) fn finish(mut self) -> [u8; 32] {
        use sha2::Digest as _;
        self.0.flush();
        self.0.hasher.finalize().into()
    }
}

// Tags keep differently shaped values from hashing alike.
const NONE: u64 = 1;
const SOME: u64 = 2;
const UNIT: u64 = 3;
const SEQ: u64 = 4;
const MAP: u64 = 5;
const STRUCT: u64 = 6;
const VARIANT: u64 = 7;
const ITEM: u64 = 8;
const END: u64 = 9;

#[derive(Debug)]
pub(crate) struct Error(String);
impl fmt::Display for Error { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(&self.0) } }
impl std::error::Error for Error {}
impl ser::Error for Error { fn custom<T: fmt::Display>(msg: T) -> Self { Self(msg.to_string()) } }

/// Sequences and maps of unknown length mark each item, so neighbours never
/// run together; known lengths are hashed up front instead.
pub(crate) struct Compound<'a, S> { hash: &'a mut Walk<S>, marked: bool }
impl<S: Sink> Compound<'_, S> {
    fn item<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        if self.marked { self.hash.word(ITEM); }
        value.serialize(&mut *self.hash)
    }
    fn end(self) -> Result<(), Error> { self.hash.word(END); Ok(()) }
}

impl<'a, S: Sink> ser::Serializer for &'a mut Walk<S> {
    type Ok = ();
    type Error = Error;
    type SerializeSeq = Compound<'a, S>;
    type SerializeTuple = Compound<'a, S>;
    type SerializeTupleStruct = Compound<'a, S>;
    type SerializeTupleVariant = Compound<'a, S>;
    type SerializeMap = Compound<'a, S>;
    type SerializeStruct = Compound<'a, S>;
    type SerializeStructVariant = Compound<'a, S>;

    fn serialize_bool(self, v: bool) -> Result<(), Error> { self.word(v as u64); Ok(()) }
    fn serialize_i8(self, v: i8) -> Result<(), Error> { self.word(v as i64 as u64); Ok(()) }
    fn serialize_i16(self, v: i16) -> Result<(), Error> { self.word(v as i64 as u64); Ok(()) }
    fn serialize_i32(self, v: i32) -> Result<(), Error> { self.word(v as i64 as u64); Ok(()) }
    fn serialize_i64(self, v: i64) -> Result<(), Error> { self.word(v as u64); Ok(()) }
    fn serialize_i128(self, v: i128) -> Result<(), Error> { self.word(v as u64); self.word((v >> 64) as u64); Ok(()) }
    fn serialize_u8(self, v: u8) -> Result<(), Error> { self.word(v as u64); Ok(()) }
    fn serialize_u16(self, v: u16) -> Result<(), Error> { self.word(v as u64); Ok(()) }
    fn serialize_u32(self, v: u32) -> Result<(), Error> { self.word(v as u64); Ok(()) }
    fn serialize_u64(self, v: u64) -> Result<(), Error> { self.word(v); Ok(()) }
    fn serialize_u128(self, v: u128) -> Result<(), Error> { self.word(v as u64); self.word((v >> 64) as u64); Ok(()) }
    fn serialize_f32(self, v: f32) -> Result<(), Error> { self.word(v.to_bits() as u64); Ok(()) }
    fn serialize_f64(self, v: f64) -> Result<(), Error> { self.word(v.to_bits()); Ok(()) }
    fn serialize_char(self, v: char) -> Result<(), Error> { self.word(v as u64); Ok(()) }
    fn serialize_str(self, v: &str) -> Result<(), Error> { self.bytes(v.as_bytes()); Ok(()) }
    fn serialize_bytes(self, v: &[u8]) -> Result<(), Error> { self.bytes(v); Ok(()) }
    fn serialize_none(self) -> Result<(), Error> { self.word(NONE); Ok(()) }
    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<(), Error> { self.word(SOME); value.serialize(self) }
    fn serialize_unit(self) -> Result<(), Error> { self.word(UNIT); Ok(()) }
    fn serialize_unit_struct(self, _name: &'static str) -> Result<(), Error> { self.word(UNIT); Ok(()) }
    fn serialize_unit_variant(self, _name: &'static str, _index: u32, variant: &'static str) -> Result<(), Error> {
        self.word(VARIANT); self.bytes(variant.as_bytes()); Ok(())
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(self, _name: &'static str, value: &T) -> Result<(), Error> { value.serialize(self) }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(self, _name: &'static str, _index: u32, variant: &'static str, value: &T) -> Result<(), Error> {
        self.word(VARIANT); self.bytes(variant.as_bytes()); value.serialize(self)
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<Compound<'a, S>, Error> {
        self.word(SEQ);
        if let Some(len) = len { self.word(len as u64); }
        Ok(Compound { hash: self, marked: len.is_none() })
    }
    fn serialize_tuple(self, len: usize) -> Result<Compound<'a, S>, Error> { self.serialize_seq(Some(len)) }
    fn serialize_tuple_struct(self, _name: &'static str, len: usize) -> Result<Compound<'a, S>, Error> { self.serialize_seq(Some(len)) }
    fn serialize_tuple_variant(self, _name: &'static str, _index: u32, variant: &'static str, len: usize) -> Result<Compound<'a, S>, Error> {
        self.word(VARIANT); self.bytes(variant.as_bytes()); self.serialize_seq(Some(len))
    }
    fn serialize_map(self, len: Option<usize>) -> Result<Compound<'a, S>, Error> {
        self.word(MAP);
        if let Some(len) = len { self.word(len as u64); }
        Ok(Compound { hash: self, marked: len.is_none() })
    }
    // Field names are hashed because skipped fields leave no other trace.
    fn serialize_struct(self, _name: &'static str, _len: usize) -> Result<Compound<'a, S>, Error> {
        self.word(STRUCT);
        Ok(Compound { hash: self, marked: false })
    }
    fn serialize_struct_variant(self, _name: &'static str, _index: u32, variant: &'static str, _len: usize) -> Result<Compound<'a, S>, Error> {
        self.word(VARIANT); self.bytes(variant.as_bytes()); self.word(STRUCT);
        Ok(Compound { hash: self, marked: false })
    }
    fn collect_str<T: fmt::Display + ?Sized>(self, value: &T) -> Result<(), Error> { self.bytes(value.to_string().as_bytes()); Ok(()) }
    fn is_human_readable(&self) -> bool { true }
}

impl<S: Sink> ser::SerializeSeq for Compound<'_, S> {
    type Ok = (); type Error = Error;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> { self.item(value) }
    fn end(self) -> Result<(), Error> { Compound::end(self) }
}
impl<S: Sink> ser::SerializeTuple for Compound<'_, S> {
    type Ok = (); type Error = Error;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> { self.item(value) }
    fn end(self) -> Result<(), Error> { Compound::end(self) }
}
impl<S: Sink> ser::SerializeTupleStruct for Compound<'_, S> {
    type Ok = (); type Error = Error;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> { self.item(value) }
    fn end(self) -> Result<(), Error> { Compound::end(self) }
}
impl<S: Sink> ser::SerializeTupleVariant for Compound<'_, S> {
    type Ok = (); type Error = Error;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> { self.item(value) }
    fn end(self) -> Result<(), Error> { Compound::end(self) }
}
impl<S: Sink> ser::SerializeMap for Compound<'_, S> {
    type Ok = (); type Error = Error;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), Error> { self.item(key) }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> { value.serialize(&mut *self.hash) }
    fn end(self) -> Result<(), Error> { Compound::end(self) }
}
impl<S: Sink> ser::SerializeStruct for Compound<'_, S> {
    type Ok = (); type Error = Error;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, key: &'static str, value: &T) -> Result<(), Error> {
        self.hash.bytes(key.as_bytes());
        value.serialize(&mut *self.hash)
    }
    fn end(self) -> Result<(), Error> { Compound::end(self) }
}
impl<S: Sink> ser::SerializeStructVariant for Compound<'_, S> {
    type Ok = (); type Error = Error;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, key: &'static str, value: &T) -> Result<(), Error> {
        self.hash.bytes(key.as_bytes());
        value.serialize(&mut *self.hash)
    }
    fn end(self) -> Result<(), Error> { Compound::end(self) }
}

#[cfg(test)]
mod tests {
    use super::Fingerprint;
    use serde::Serialize;
    fn hash<T: Serialize>(value: &T) -> u64 { let mut f = Fingerprint::new(); f.add(value).unwrap(); f.finish() }

    #[derive(Serialize)]
    struct Skips { #[serde(skip_serializing_if = "Option::is_none")] a: Option<u8>, #[serde(skip_serializing_if = "Option::is_none")] b: Option<u8> }

    #[test]
    fn shape_and_content_distinguish_values() {
        assert_eq!(hash(&vec![1.5f32, 2.0]), hash(&vec![1.5f32, 2.0]));
        assert_ne!(hash(&vec![1.5f32, 2.0]), hash(&vec![2.0f32, 1.5]));
        assert_ne!(hash(&(vec![1u8], vec![2u8, 3])), hash(&(vec![1u8, 2], vec![3u8])));
        assert_ne!(hash(&("ab", "c")), hash(&("a", "bc")));
        assert_ne!(hash(&Skips { a: Some(1), b: None }), hash(&Skips { a: None, b: Some(1) }));
        assert_ne!(hash(&Some(0u8)), hash(&None::<u8>));
        assert_ne!(hash(&0.0f32), hash(&-0.0f32));
    }

    #[test]
    fn digest_distinguishes_shapes_across_its_buffer() {
        fn digest<T: Serialize>(value: &T) -> [u8; 32] { let mut d = super::Digest::new(); d.add(value).unwrap(); d.finish() }
        let long = "x".repeat(40_000);
        assert_eq!(digest(&(long.as_str(), 1u8)), digest(&(long.as_str(), 1u8)));
        assert_ne!(digest(&(long.as_str(), 1u8)), digest(&(&long[1..], 1u8)));
        assert_ne!(digest(&("ab", "c")), digest(&("a", "bc")));
        assert_ne!(digest(&vec![1u64; 5000]), digest(&vec![1u64; 5001]));
    }
}
