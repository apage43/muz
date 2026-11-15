//! A fast in-process content hash of anything `Serialize`. It walks the same
//! serde shape that JSON would, hashing primitives' bits, string bytes,
//! lengths, field names and variant tags. It never formats text, so a large
//! description (resolved SFZ regions run to hundreds of thousands of fields)
//! hashes several times faster than its JSON. Not stable across builds, and
//! not a security hash: it is only a stale-description guard.
use serde::ser::{self, Serialize};
use std::fmt;

const K0: u64 = 0x9E37_79B9_7F4A_7C15;
const K1: u64 = 0xA076_1D64_78BD_642F;

#[inline(always)]
fn fold(a: u64, b: u64) -> u64 {
    let product = (a as u128).wrapping_mul(b as u128);
    (product as u64) ^ ((product >> 64) as u64)
}

pub(crate) struct Fingerprint { state: u64, count: u64 }

impl Fingerprint {
    pub(crate) fn new() -> Self { Self { state: K0, count: 0 } }
    #[inline(always)]
    fn word(&mut self, w: u64) {
        self.count = self.count.wrapping_add(1);
        self.state = fold(self.state ^ w, K1 ^ self.count.wrapping_mul(K0));
    }
    fn bytes(&mut self, bytes: &[u8]) {
        self.word(bytes.len() as u64);
        let mut chunks = bytes.chunks_exact(8);
        for chunk in &mut chunks { self.word(u64::from_le_bytes(chunk.try_into().expect("8 bytes"))); }
        let rest = chunks.remainder();
        if !rest.is_empty() {
            let mut last = [0u8; 8];
            last[..rest.len()].copy_from_slice(rest);
            self.word(u64::from_le_bytes(last));
        }
    }
    pub(crate) fn add<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        value.serialize(&mut *self)
    }
    pub(crate) fn finish(&self) -> u64 { fold(self.state ^ self.count, K0) }
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
pub(crate) struct Compound<'a> { hash: &'a mut Fingerprint, marked: bool }
impl Compound<'_> {
    fn item<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        if self.marked { self.hash.word(ITEM); }
        value.serialize(&mut *self.hash)
    }
    fn end(self) -> Result<(), Error> { self.hash.word(END); Ok(()) }
}

impl<'a> ser::Serializer for &'a mut Fingerprint {
    type Ok = ();
    type Error = Error;
    type SerializeSeq = Compound<'a>;
    type SerializeTuple = Compound<'a>;
    type SerializeTupleStruct = Compound<'a>;
    type SerializeTupleVariant = Compound<'a>;
    type SerializeMap = Compound<'a>;
    type SerializeStruct = Compound<'a>;
    type SerializeStructVariant = Compound<'a>;

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
    fn serialize_seq(self, len: Option<usize>) -> Result<Compound<'a>, Error> {
        self.word(SEQ);
        if let Some(len) = len { self.word(len as u64); }
        Ok(Compound { hash: self, marked: len.is_none() })
    }
    fn serialize_tuple(self, len: usize) -> Result<Compound<'a>, Error> { self.serialize_seq(Some(len)) }
    fn serialize_tuple_struct(self, _name: &'static str, len: usize) -> Result<Compound<'a>, Error> { self.serialize_seq(Some(len)) }
    fn serialize_tuple_variant(self, _name: &'static str, _index: u32, variant: &'static str, len: usize) -> Result<Compound<'a>, Error> {
        self.word(VARIANT); self.bytes(variant.as_bytes()); self.serialize_seq(Some(len))
    }
    fn serialize_map(self, len: Option<usize>) -> Result<Compound<'a>, Error> {
        self.word(MAP);
        if let Some(len) = len { self.word(len as u64); }
        Ok(Compound { hash: self, marked: len.is_none() })
    }
    // Field names are hashed because skipped fields leave no other trace.
    fn serialize_struct(self, _name: &'static str, _len: usize) -> Result<Compound<'a>, Error> {
        self.word(STRUCT);
        Ok(Compound { hash: self, marked: false })
    }
    fn serialize_struct_variant(self, _name: &'static str, _index: u32, variant: &'static str, _len: usize) -> Result<Compound<'a>, Error> {
        self.word(VARIANT); self.bytes(variant.as_bytes()); self.word(STRUCT);
        Ok(Compound { hash: self, marked: false })
    }
    fn collect_str<T: fmt::Display + ?Sized>(self, value: &T) -> Result<(), Error> { self.bytes(value.to_string().as_bytes()); Ok(()) }
    fn is_human_readable(&self) -> bool { true }
}

impl ser::SerializeSeq for Compound<'_> {
    type Ok = (); type Error = Error;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> { self.item(value) }
    fn end(self) -> Result<(), Error> { Compound::end(self) }
}
impl ser::SerializeTuple for Compound<'_> {
    type Ok = (); type Error = Error;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> { self.item(value) }
    fn end(self) -> Result<(), Error> { Compound::end(self) }
}
impl ser::SerializeTupleStruct for Compound<'_> {
    type Ok = (); type Error = Error;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> { self.item(value) }
    fn end(self) -> Result<(), Error> { Compound::end(self) }
}
impl ser::SerializeTupleVariant for Compound<'_> {
    type Ok = (); type Error = Error;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> { self.item(value) }
    fn end(self) -> Result<(), Error> { Compound::end(self) }
}
impl ser::SerializeMap for Compound<'_> {
    type Ok = (); type Error = Error;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), Error> { self.item(key) }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> { value.serialize(&mut *self.hash) }
    fn end(self) -> Result<(), Error> { Compound::end(self) }
}
impl ser::SerializeStruct for Compound<'_> {
    type Ok = (); type Error = Error;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, key: &'static str, value: &T) -> Result<(), Error> {
        self.hash.bytes(key.as_bytes());
        value.serialize(&mut *self.hash)
    }
    fn end(self) -> Result<(), Error> { Compound::end(self) }
}
impl ser::SerializeStructVariant for Compound<'_> {
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
}
