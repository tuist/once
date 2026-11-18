//! Data-format primitives for property lists and CMS (Cryptographic Message
//! Syntax) `SignedData` messages. Both stay format-level: what a document
//! means is interpreted by the target kinds that read it.

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine as _;
use serde_json::Value as JsonValue;
use starlark::values::dict::DictRef;
use starlark::values::float::StarlarkFloat;
use starlark::values::list::ListRef;
use starlark::values::tuple::TupleRef;
use starlark::values::{UnpackValue, Value};

/// The `id-signedData` object identifier, 1.2.840.113549.1.7.2.
const SIGNED_DATA_OID: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x07, 0x02];
const MAX_BER_DEPTH: usize = 32;
const MAX_PLIST_DEPTH: usize = 256;

const TAG_OCTET_STRING: u8 = 0x04;
const TAG_OCTET_STRING_CONSTRUCTED: u8 = 0x24;
const TAG_OID: u8 = 0x06;
const TAG_SEQUENCE: u8 = 0x30;
const TAG_EXPLICIT_0: u8 = 0xa0;

/// Return the encapsulated content of a CMS `SignedData` message in DER or
/// BER encoding. The signature is not verified.
pub(super) fn cms_signed_content(bytes: &[u8]) -> Result<Vec<u8>> {
    let (content_info, _) = element(bytes, 0).context("reading CMS ContentInfo")?;
    expect_tag(&content_info, TAG_SEQUENCE, "ContentInfo")?;
    let mut fields = children(&content_info)?.into_iter();
    let content_type = fields
        .next()
        .ok_or_else(|| anyhow!("CMS ContentInfo is empty"))?;
    expect_tag(&content_type, TAG_OID, "ContentInfo contentType")?;
    if content_type.content != SIGNED_DATA_OID {
        bail!("CMS message is not SignedData");
    }
    let explicit = fields
        .next()
        .ok_or_else(|| anyhow!("CMS SignedData has no content"))?;
    expect_tag(&explicit, TAG_EXPLICIT_0, "ContentInfo content")?;
    let signed_data = only_child(&explicit, Some(TAG_SEQUENCE), "SignedData")?;
    // version, digestAlgorithms, encapContentInfo, ...
    let encapsulated = children(&signed_data)?
        .into_iter()
        .nth(2)
        .ok_or_else(|| anyhow!("CMS SignedData has no encapsulated content"))?;
    expect_tag(&encapsulated, TAG_SEQUENCE, "EncapsulatedContentInfo")?;
    let content = children(&encapsulated)?
        .into_iter()
        .nth(1)
        .ok_or_else(|| anyhow!("CMS SignedData is detached and carries no content"))?;
    expect_tag(&content, TAG_EXPLICIT_0, "eContent")?;
    let octets = only_child(&content, None, "eContent")?;
    let mut out = Vec::new();
    octet_string(&octets, &mut out, 0)?;
    Ok(out)
}

/// Decode an XML property list into JSON-shaped data. Dates become RFC 3339
/// strings and data becomes standard base64 strings.
///
/// Element nesting is measured before the property list is parsed, so a
/// hostile document is rejected before a tree is built and later dropped.
pub(super) fn plist_to_json(source: &str) -> Result<JsonValue> {
    check_xml_depth(source)?;
    let value =
        plist::Value::from_reader_xml(source.as_bytes()).context("decoding property list")?;
    plist_value_to_json(value)
}

fn check_xml_depth(source: &str) -> Result<()> {
    let mut reader = quick_xml::Reader::from_str(source);
    let mut depth = 0usize;
    loop {
        match reader.read_event().context("decoding property list")? {
            quick_xml::events::Event::Start(_) => {
                depth += 1;
                // The `<plist>` root element wraps the outermost value.
                if depth > MAX_PLIST_DEPTH + 1 {
                    bail!("property list is nested more than {MAX_PLIST_DEPTH} levels deep");
                }
            }
            quick_xml::events::Event::End(_) => depth = depth.saturating_sub(1),
            quick_xml::events::Event::Eof => return Ok(()),
            _ => {}
        }
    }
}

fn plist_value_to_json(value: plist::Value) -> Result<JsonValue> {
    Ok(match value {
        plist::Value::Array(items) => JsonValue::Array(
            items
                .into_iter()
                .map(plist_value_to_json)
                .collect::<Result<_>>()?,
        ),
        plist::Value::Dictionary(entries) => JsonValue::Object(
            entries
                .into_iter()
                .map(|(key, value)| Ok((key, plist_value_to_json(value)?)))
                .collect::<Result<_>>()?,
        ),
        plist::Value::Boolean(value) => JsonValue::Bool(value),
        plist::Value::Data(bytes) => {
            JsonValue::String(base64::engine::general_purpose::STANDARD.encode(bytes))
        }
        plist::Value::Date(date) => JsonValue::String(date.to_xml_format()),
        plist::Value::Real(value) => {
            serde_json::Number::from_f64(value).map_or(JsonValue::Null, JsonValue::Number)
        }
        plist::Value::Integer(value) => match value.as_signed() {
            Some(signed) => JsonValue::from(signed),
            None => JsonValue::from(value.as_unsigned().unwrap_or_default()),
        },
        plist::Value::String(value) => JsonValue::String(value),
        plist::Value::Uid(uid) => JsonValue::from(uid.get()),
        _ => bail!("property list contains an unsupported value type"),
    })
}

/// Encode a Starlark value as an XML property list. Dictionaries keep their
/// insertion order. Values a property list cannot represent fail rather than
/// being converted to strings.
pub(super) fn starlark_to_plist_xml(value: Value<'_>) -> Result<String> {
    let mut out = Vec::new();
    starlark_to_plist(value, 0)?
        .to_writer_xml(&mut out)
        .context("encoding property list")?;
    out.push(b'\n');
    String::from_utf8(out).context("property list encoder produced invalid UTF-8")
}

fn starlark_to_plist(value: Value<'_>, depth: usize) -> Result<plist::Value> {
    if depth >= MAX_PLIST_DEPTH {
        bail!("value is nested more than {MAX_PLIST_DEPTH} levels deep");
    }
    if let Some(value) = value.unpack_bool() {
        return Ok(plist::Value::Boolean(value));
    }
    if let Some(value) = value.unpack_str() {
        return Ok(plist::Value::String(value.to_string()));
    }
    if let Some(float) = StarlarkFloat::unpack_value(value).map_err(|error| anyhow!("{error}"))? {
        return Ok(plist::Value::Real(float.0));
    }
    if let Ok(Some(integer)) = i64::unpack_value(value) {
        return Ok(plist::Value::Integer(integer.into()));
    }
    let items = ListRef::from_value(value)
        .map(|list| list.content().to_vec())
        .or_else(|| TupleRef::from_value(value).map(|tuple| tuple.content().to_vec()));
    if let Some(items) = items {
        return Ok(plist::Value::Array(
            items
                .into_iter()
                .map(|item| starlark_to_plist(item, depth + 1))
                .collect::<Result<_>>()?,
        ));
    }
    if let Some(dict) = DictRef::from_value(value) {
        let mut entries = plist::Dictionary::new();
        for (key, item) in dict.iter() {
            let key = key.unpack_str().ok_or_else(|| {
                anyhow!("property list dictionary keys must be strings, got `{key}`")
            })?;
            entries.insert(key.to_string(), starlark_to_plist(item, depth + 1)?);
        }
        return Ok(plist::Value::Dictionary(entries));
    }
    bail!(
        "property lists cannot represent a value of type `{}`",
        value.get_type()
    )
}

struct Element<'a> {
    tag: u8,
    /// The element's content octets, excluding end-of-contents markers of an
    /// indefinite-length encoding.
    content: &'a [u8],
}

fn expect_tag(element: &Element<'_>, tag: u8, name: &str) -> Result<()> {
    if element.tag == tag {
        Ok(())
    } else {
        bail!(
            "CMS {name} has tag 0x{:02x}, expected 0x{tag:02x}",
            element.tag
        )
    }
}

/// The first child of a constructed element, of `tag` when given.
fn only_child<'a>(parent: &Element<'a>, tag: Option<u8>, name: &str) -> Result<Element<'a>> {
    let child = children(parent)?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("CMS {name} is empty"))?;
    if let Some(tag) = tag {
        expect_tag(&child, tag, name)?;
    }
    Ok(child)
}

fn children<'a>(parent: &Element<'a>) -> Result<Vec<Element<'a>>> {
    if parent.tag & 0x20 == 0 {
        bail!("CMS element 0x{:02x} is not constructed", parent.tag);
    }
    let mut rest = parent.content;
    let mut out = Vec::new();
    while !rest.is_empty() {
        let (child, remaining) = element(rest, 1)?;
        out.push(child);
        rest = remaining;
    }
    Ok(out)
}

fn octet_string(element: &Element<'_>, out: &mut Vec<u8>, depth: usize) -> Result<()> {
    if depth > MAX_BER_DEPTH {
        bail!("CMS content is nested too deeply");
    }
    match element.tag {
        TAG_OCTET_STRING => out.extend_from_slice(element.content),
        TAG_OCTET_STRING_CONSTRUCTED => {
            for chunk in children(element)? {
                octet_string(&chunk, out, depth + 1)?;
            }
        }
        tag => bail!("CMS eContent has tag 0x{tag:02x}, expected an OCTET STRING"),
    }
    Ok(())
}

/// Read one BER element from the front of `bytes`, returning it and the
/// bytes after it.
fn element(bytes: &[u8], depth: usize) -> Result<(Element<'_>, &[u8])> {
    if depth > MAX_BER_DEPTH {
        bail!("CMS message is nested too deeply");
    }
    let truncated = || anyhow!("CMS message is truncated");
    let &tag = bytes.first().ok_or_else(truncated)?;
    if tag & 0x1f == 0x1f {
        bail!("CMS message uses an unsupported high tag number");
    }
    let &first = bytes.get(1).ok_or_else(truncated)?;
    let body = &bytes[2..];
    if first == 0x80 {
        if tag & 0x20 == 0 {
            bail!("CMS primitive element uses indefinite length");
        }
        let mut rest = body;
        loop {
            if rest.starts_with(&[0, 0]) {
                let consumed = body.len() - rest.len();
                return Ok((
                    Element {
                        tag,
                        content: &body[..consumed],
                    },
                    &rest[2..],
                ));
            }
            if rest.is_empty() {
                return Err(truncated());
            }
            rest = element(rest, depth + 1)?.1;
        }
    }
    let (length, body) = if first & 0x80 == 0 {
        (usize::from(first), body)
    } else {
        let count = usize::from(first & 0x7f);
        if count > std::mem::size_of::<usize>() || body.len() < count {
            return Err(truncated());
        }
        let length = body[..count]
            .iter()
            .fold(0usize, |length, &byte| (length << 8) | usize::from(byte));
        (length, &body[count..])
    };
    if body.len() < length {
        return Err(truncated());
    }
    Ok((
        Element {
            tag,
            content: &body[..length],
        },
        &body[length..],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn der(tag: u8, content: &[u8]) -> Vec<u8> {
        let mut out = vec![tag];
        let length = content.len();
        if length < 0x80 {
            out.push(u8::try_from(length).unwrap());
        } else {
            let bytes = length.to_be_bytes();
            let bytes = &bytes[bytes.iter().position(|&byte| byte != 0).unwrap()..];
            out.push(0x80 | u8::try_from(bytes.len()).unwrap());
            out.extend_from_slice(bytes);
        }
        out.extend_from_slice(content);
        out
    }

    fn indefinite(tag: u8, parts: &[Vec<u8>]) -> Vec<u8> {
        let mut out = vec![tag, 0x80];
        for part in parts {
            out.extend_from_slice(part);
        }
        out.extend_from_slice(&[0, 0]);
        out
    }

    fn signed_data(encapsulated_content: Vec<u8>, definite: bool) -> Vec<u8> {
        let content_type = der(
            TAG_OID,
            &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x07, 0x01],
        );
        let version = der(0x02, &[1]);
        let digests = der(0x31, &der(TAG_SEQUENCE, &der(TAG_OID, &[0x60, 0x86, 0x48])));
        let signer_infos = der(0x31, &[]);
        let build = |tag: u8, parts: Vec<Vec<u8>>| {
            if definite {
                der(tag, &parts.concat())
            } else {
                indefinite(tag, &parts)
            }
        };
        let encapsulated = build(
            TAG_SEQUENCE,
            vec![
                content_type,
                build(TAG_EXPLICIT_0, vec![encapsulated_content]),
            ],
        );
        let signed = build(
            TAG_SEQUENCE,
            vec![version, digests, encapsulated, signer_infos],
        );
        build(
            TAG_SEQUENCE,
            vec![
                der(TAG_OID, SIGNED_DATA_OID),
                build(TAG_EXPLICIT_0, vec![signed]),
            ],
        )
    }

    #[test]
    fn signed_content_is_read_from_definite_and_indefinite_encodings() {
        let text = "<plist>".to_string() + &"x".repeat(300) + "</plist>";
        let definite = signed_data(der(TAG_OCTET_STRING, text.as_bytes()), true);
        assert_eq!(cms_signed_content(&definite).unwrap(), text.as_bytes());

        let (head, tail) = text.as_bytes().split_at(5);
        let chunked = indefinite(
            TAG_OCTET_STRING_CONSTRUCTED,
            &[der(TAG_OCTET_STRING, head), der(TAG_OCTET_STRING, tail)],
        );
        assert_eq!(
            cms_signed_content(&signed_data(chunked, false)).unwrap(),
            text.as_bytes()
        );
    }

    #[test]
    fn non_signed_data_and_truncated_messages_are_rejected() {
        let mut message = signed_data(der(TAG_OCTET_STRING, b"content"), true);
        let other = der(
            TAG_SEQUENCE,
            &[
                der(
                    TAG_OID,
                    &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x07, 0x03],
                ),
                der(TAG_EXPLICIT_0, &[]),
            ]
            .concat(),
        );
        assert!(cms_signed_content(&other)
            .unwrap_err()
            .to_string()
            .contains("not SignedData"));
        message.truncate(message.len() - 3);
        assert!(format!("{:#}", cms_signed_content(&message).unwrap_err()).contains("truncated"));
        assert!(cms_signed_content(b"<?xml version").is_err());
    }

    #[test]
    fn property_lists_decode_into_json_shapes() {
        let source = br#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>name</key><string>a &amp; b</string>
<key>flag</key><false/>
<key>count</key><integer>-3</integer>
<key>list</key><array><string>x</string><true/></array>
<key>blob</key><data>AAEC</data>
<key>when</key><date>2027-01-02T03:04:05Z</date>
</dict></plist>"#;
        let json = plist_to_json(std::str::from_utf8(source).unwrap()).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "name": "a & b", "flag": false, "count": -3, "list": ["x", true],
                "blob": "AAEC", "when": "2027-01-02T03:04:05Z",
            })
        );
    }

    fn encode(expression: &str) -> std::result::Result<String, String> {
        use starlark::environment::Module;
        use starlark::eval::Evaluator;
        use starlark::syntax::{AstModule, Dialect};
        Module::with_temp_heap(|module| {
            let source = format!("value = plist_encode({expression})");
            let ast = AstModule::parse("test.star", source, &Dialect::Standard)
                .map_err(|error| error.to_string())?;
            Evaluator::new(&module)
                .eval_module(ast, &crate::analysis::globals_for_prelude())
                .map_err(|error| error.to_string())?;
            Ok(module
                .get("value")
                .unwrap()
                .unpack_str()
                .unwrap()
                .to_string())
        })
    }

    #[test]
    fn encoding_keeps_types_and_order_and_rejects_what_plists_cannot_hold() {
        let encoded = encode(r#"{"z": "a & b", "big": 3000000000, "real": 1.5, "flag": False, "items": ("x", [True])}"#)
            .unwrap();
        assert!(
            encoded.contains("<key>z</key>\n\t<string>a &amp; b</string>\n\t<key>big</key>\n\t<integer>3000000000</integer>\n\t<key>real</key>\n\t<real>1.5</real>\n\t<key>flag</key>\n\t<false/>"),
            "{encoded}"
        );
        assert!(encoded.contains("<string>x</string>"), "{encoded}");
        assert!(encoded.contains("<true/>"), "{encoded}");
        for (expression, expected) in [
            (r#"{"x": None}"#, "type `NoneType`"),
            ("{1: True}", "keys must be strings"),
            ("{'x': len}", "cannot represent"),
        ] {
            let error = encode(expression).unwrap_err();
            assert!(error.contains(expected), "{expression}: {error}");
        }
    }

    #[test]
    fn deeply_nested_property_lists_fail_without_building_the_tree() {
        let depth = 200_000;
        let source = format!(
            "<plist version=\"1.0\">{}{}</plist>",
            "<array>".repeat(depth),
            "</array>".repeat(depth)
        );
        let error = plist_to_json(&source).unwrap_err();
        assert!(
            error.to_string().contains("nested more than 256"),
            "{error}"
        );
        let nested = format!(
            "<plist version=\"1.0\">{}{}</plist>",
            "<array>".repeat(256),
            "</array>".repeat(256)
        );
        assert!(plist_to_json(&nested).is_ok());
    }
}
