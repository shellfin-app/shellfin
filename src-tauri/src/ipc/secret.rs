//! `Secret<T>` (§4.1, §9.2): inbound-only wrapper for passwords, passphrases and kbd-int answers.
//!
//! - `Deserialize` only — there is deliberately **no** `Serialize` impl, so any outbound DTO that
//!   contains a `Secret` fails to compile.
//! - Zeroized on drop (`zeroize::Zeroizing`).
//! - `Debug` prints `***`, so `{:?}` of any inbound DTO never leaks the value into logs.
//! - TypeScript: renders exactly as `T` (manual `impl TS`).

use std::fmt;

use serde::{Deserialize, Deserializer};
use ts_rs::{Config, TS, TypeVisitor};
use zeroize::{Zeroize, Zeroizing};

pub struct Secret<T: Zeroize>(Zeroizing<T>);

impl<T: Zeroize> Secret<T> {
    pub fn new(value: T) -> Self {
        Self(Zeroizing::new(value))
    }

    /// Borrow the secret value. Callers must not copy it into non-zeroizing storage or logs.
    pub fn expose(&self) -> &T {
        &self.0
    }
}

impl<T: Zeroize> fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

impl<'de, T: Zeroize + Deserialize<'de>> Deserialize<'de> for Secret<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        T::deserialize(deserializer).map(Self::new)
    }
}

/// Transparent: `Secret<T>` is typed as `T` on the TypeScript side.
impl<T: Zeroize + TS> TS for Secret<T> {
    type WithoutGenerics = Self;
    type OptionInnerType = Self;

    fn name(cfg: &Config) -> String {
        T::name(cfg)
    }

    fn inline(cfg: &Config) -> String {
        T::inline(cfg)
    }

    fn inline_flattened(cfg: &Config) -> String {
        T::inline_flattened(cfg)
    }

    fn visit_dependencies(v: &mut impl TypeVisitor)
    where
        Self: 'static,
    {
        T::visit_dependencies(v);
    }

    fn visit_generics(v: &mut impl TypeVisitor)
    where
        Self: 'static,
    {
        T::visit_generics(v);
        v.visit::<T>();
    }

    fn decl(_: &Config) -> String {
        panic!("Secret<T> is a transparent wrapper and cannot be declared")
    }

    fn decl_concrete(_: &Config) -> String {
        panic!("Secret<T> is a transparent wrapper and cannot be declared")
    }
}

/// User text that must never reach a log (a bookmark's `startupCommand`). Unlike
/// [`Secret`] it travels both ways and is stored in `bookmarks.json`; only `Debug` is masked.
/// Serialized as a plain string; typed as `string` in TypeScript.
#[derive(Clone, PartialEq, Eq, serde::Serialize, Deserialize)]
#[serde(transparent)]
pub struct SensitiveText(String);

impl SensitiveText {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The text. Callers must not log it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SensitiveText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

/// Transparent: `SensitiveText` is typed as `string` on the TypeScript side.
impl TS for SensitiveText {
    type WithoutGenerics = Self;
    type OptionInnerType = Self;

    fn name(cfg: &Config) -> String {
        String::name(cfg)
    }

    fn inline(cfg: &Config) -> String {
        String::inline(cfg)
    }

    fn inline_flattened(cfg: &Config) -> String {
        String::inline_flattened(cfg)
    }

    fn visit_dependencies(_: &mut impl TypeVisitor)
    where
        Self: 'static,
    {
    }

    fn visit_generics(_: &mut impl TypeVisitor)
    where
        Self: 'static,
    {
    }

    fn decl(_: &Config) -> String {
        panic!("SensitiveText is a transparent wrapper and cannot be declared")
    }

    fn decl_concrete(_: &Config) -> String {
        panic!("SensitiveText is a transparent wrapper and cannot be declared")
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn sensitive_text_is_transparent_and_masked() {
        let s: SensitiveText = serde_json::from_str(r#""cd /srv && ls""#).unwrap();
        assert_eq!(s.expose(), "cd /srv && ls");
        assert_eq!(format!("{s:?}"), "***");
        assert_eq!(format!("{:?}", Some(s.clone())), "Some(***)");
        assert_eq!(serde_json::to_string(&s).unwrap(), r#""cd /srv && ls""#);
        let cfg = Config::default();
        assert_eq!(<SensitiveText as TS>::name(&cfg), "string");
        assert_eq!(<Option<SensitiveText> as TS>::name(&cfg), "string | null");
    }

    /// Compile-time proof that `Secret<T>` does not implement `Serialize` (same trick as
    /// `static_assertions::assert_not_impl_any!`): if it did, `check` would be ambiguous.
    #[test]
    fn secret_is_not_serialize() {
        trait AmbiguousIfSerialize<A> {
            fn check() {}
        }
        impl<T: ?Sized> AmbiguousIfSerialize<()> for T {}
        #[allow(dead_code)]
        struct Invalid;
        impl<T: ?Sized + serde::Serialize> AmbiguousIfSerialize<Invalid> for T {}
        <Secret<String> as AmbiguousIfSerialize<_>>::check();
    }

    #[test]
    fn debug_is_masked() {
        let s: Secret<String> = serde_json::from_str(r#""hunter2""#).unwrap();
        assert_eq!(s.expose(), "hunter2");
        assert_eq!(format!("{s:?}"), "***");
        let v: Vec<Secret<String>> = serde_json::from_str(r#"["123456","abc"]"#).unwrap();
        let dbg = format!("{v:?}");
        assert!(!dbg.contains("123456") && !dbg.contains("abc"), "{dbg}");
    }

    #[test]
    fn typescript_is_transparent() {
        let cfg = Config::default();
        assert_eq!(<Secret<String> as TS>::name(&cfg), "string");
        assert_eq!(<Option<Secret<String>> as TS>::name(&cfg), "string | null");
        assert_eq!(<Vec<Secret<String>> as TS>::name(&cfg), "Array<string>");
    }
}
