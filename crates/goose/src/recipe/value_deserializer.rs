use serde::de::{DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::forward_to_deserialize_any;
use serde_yaml::mapping::Iter as MappingIter;
use serde_yaml::Value;
use std::slice;

pub(super) struct RecipeValueDeserializer<'de> {
    value: &'de Value,
}

impl<'de> RecipeValueDeserializer<'de> {
    pub(super) fn new(value: &'de Value) -> Self {
        Self { value }
    }

    fn deserialize_string_value<V>(self, visitor: V) -> Result<V::Value, serde_yaml::Error>
    where
        V: Visitor<'de>,
    {
        match self.value {
            Value::String(value) => visitor.visit_borrowed_str(value),
            Value::Number(value) => visitor.visit_string(value.to_string()),
            Value::Bool(value) => visitor.visit_string(value.to_string()),
            Value::Null => visitor.visit_string("null".to_string()),
            Value::Tagged(tagged) => Self::new(&tagged.value).deserialize_string_value(visitor),
            other => serde::Deserializer::deserialize_string(other, visitor),
        }
    }
}

impl<'de> Deserializer<'de> for RecipeValueDeserializer<'de> {
    type Error = serde_yaml::Error;

    fn deserialize_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        match self.value {
            Value::Sequence(sequence) => visitor.visit_seq(RecipeSeqAccess {
                iter: sequence.iter(),
            }),
            Value::Mapping(mapping) => visitor.visit_map(RecipeMapAccess {
                iter: mapping.iter(),
                value: None,
            }),
            Value::Tagged(tagged) => Self::new(&tagged.value).deserialize_any(visitor),
            value => serde::Deserializer::deserialize_any(value, visitor),
        }
    }

    fn deserialize_char<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_string_value(visitor)
    }

    fn deserialize_str<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_string_value(visitor)
    }

    fn deserialize_string<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_string_value(visitor)
    }

    fn deserialize_option<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        match self.value {
            Value::Null => visitor.visit_none(),
            _ => visitor.visit_some(self),
        }
    }

    fn deserialize_seq<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        match self.value {
            Value::Sequence(sequence) => visitor.visit_seq(RecipeSeqAccess {
                iter: sequence.iter(),
            }),
            Value::Tagged(tagged) => Self::new(&tagged.value).deserialize_seq(visitor),
            value => serde::Deserializer::deserialize_seq(value, visitor),
        }
    }

    fn deserialize_tuple<V>(self, _len: usize, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_seq(visitor)
    }

    fn deserialize_tuple_struct<V>(
        self,
        _name: &'static str,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_seq(visitor)
    }

    fn deserialize_map<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        match self.value {
            Value::Mapping(mapping) => visitor.visit_map(RecipeMapAccess {
                iter: mapping.iter(),
                value: None,
            }),
            Value::Tagged(tagged) => Self::new(&tagged.value).deserialize_map(visitor),
            value => serde::Deserializer::deserialize_map(value, visitor),
        }
    }

    fn deserialize_struct<V>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_map(visitor)
    }

    fn deserialize_enum<V>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        serde::Deserializer::deserialize_enum(self.value, name, variants, visitor)
    }

    fn deserialize_newtype_struct<V>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_newtype_struct(self)
    }

    forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 bytes byte_buf
        unit unit_struct identifier ignored_any
    }
}

struct RecipeSeqAccess<'de> {
    iter: slice::Iter<'de, Value>,
}

impl<'de> SeqAccess<'de> for RecipeSeqAccess<'de> {
    type Error = serde_yaml::Error;

    fn next_element_seed<T>(&mut self, seed: T) -> Result<Option<T::Value>, Self::Error>
    where
        T: DeserializeSeed<'de>,
    {
        self.iter
            .next()
            .map(|value| seed.deserialize(RecipeValueDeserializer::new(value)))
            .transpose()
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.iter.len())
    }
}

struct RecipeMapAccess<'de> {
    iter: MappingIter<'de>,
    value: Option<&'de Value>,
}

impl<'de> MapAccess<'de> for RecipeMapAccess<'de> {
    type Error = serde_yaml::Error;

    fn next_key_seed<K>(&mut self, seed: K) -> Result<Option<K::Value>, Self::Error>
    where
        K: DeserializeSeed<'de>,
    {
        let Some((key, value)) = self.iter.next() else {
            return Ok(None);
        };
        self.value = Some(value);
        seed.deserialize(RecipeValueDeserializer::new(key))
            .map(Some)
    }

    fn next_value_seed<V>(&mut self, seed: V) -> Result<V::Value, Self::Error>
    where
        V: DeserializeSeed<'de>,
    {
        let value = self.value.take().expect("value requested before map key");
        seed.deserialize(RecipeValueDeserializer::new(value))
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.iter.len())
    }
}
