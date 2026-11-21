use super::*;
use prost_types::field_descriptor_proto::{Label, Type};
use prost_types::{FieldDescriptorProto, FileDescriptorProto};

fn varint_bytes(mut v: u64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte);
            return out;
        }
        out.push(byte | 0x80);
    }
}

fn ld(field: u32, bytes: &[u8]) -> Vec<u8> {
    let mut out = varint_bytes(u64::from(field) << 3 | 2);
    out.extend(varint_bytes(bytes.len() as u64));
    out.extend_from_slice(bytes);
    out
}

/// A raw field, with `media` written as protoc writes `[(anemnez.extension.v1.anemnez_field).media = "…"]`.
fn field(name: &str, number: i32, ty: Type, media: Option<&str>) -> Vec<u8> {
    let mut bytes = FieldDescriptorProto {
        name: Some(name.into()),
        number: Some(number),
        r#type: Some(ty as i32),
        label: Some(Label::Optional as i32),
        ..Default::default()
    }
    .encode_to_vec();
    if let Some(media) = media {
        let options = wire::FieldOptions { media: media.into() }.encode_to_vec();
        bytes.extend(ld(8, &ld(FIELD_OPTIONS_FIELD, &options)));
    }
    bytes
}

/// A raw message of `fields`, with `nested` messages inside it.
fn message(name: &str, fields: Vec<Vec<u8>>, nested: Vec<Vec<u8>>) -> Vec<u8> {
    let mut bytes = prost_types::DescriptorProto { name: Some(name.into()), ..Default::default() }.encode_to_vec();
    for f in fields {
        bytes.extend(ld(2, &f));
    }
    for n in nested {
        bytes.extend(ld(3, &n));
    }
    bytes
}

fn set(messages: Vec<Vec<u8>>) -> Vec<u8> {
    let mut file = FileDescriptorProto {
        name: Some("look.proto".into()),
        package: Some("minion.v1".into()),
        ..Default::default()
    }
    .encode_to_vec();
    for m in messages {
        file.extend(ld(4, &m));
    }
    ld(1, &file)
}

/// The option round-trips: a tagged bytes field is found with its media, by its message's full name, nested
/// messages too, and an untagged one isn't.
#[test]
fn a_tagged_bytes_field_says_its_media() {
    let fds = set(vec![message(
        "CaptureReply",
        vec![
            field("png", 1, Type::Bytes, Some("image/png")),
            field("raw", 2, Type::Bytes, None),
            field("scale", 3, Type::Double, None),
        ],
        vec![message("Thumb", vec![field("jpeg", 1, Type::Bytes, Some("image/jpeg"))], vec![])],
    )]);
    assert_eq!(media_of(&fds, ".minion.v1.CaptureReply"), Ok(vec![("png".into(), Media::Png)]));
    assert_eq!(media_of(&fds, ".minion.v1.CaptureReply.Thumb"), Ok(vec![("jpeg".into(), Media::Jpeg)]));
    assert_eq!(media_of(&fds, ".minion.v1.Other"), Ok(vec![]));
    for m in Media::ALL {
        assert_eq!(Media::from_mime(m.mime()), Some(m));
        assert!(m.is_image());
    }
}

/// The option on a field that isn't bytes refuses the descriptor, naming the field.
#[test]
fn media_on_a_field_that_is_not_bytes_is_refused() {
    let fds = set(vec![message("CaptureReply", vec![field("title", 1, Type::String, Some("image/png"))], vec![])]);
    let refused = media_fields(&fds).expect_err("refused");
    assert_eq!(refused, vec![DescriptorRefusal::MediaNotBytes("minion.v1.CaptureReply.title".into())]);
    assert_eq!(refused[0].name(), "media-not-bytes");
}

/// A media outside the closed set refuses it by name, with what it said; nothing is guessed.
#[test]
fn an_unknown_media_is_refused() {
    for said in ["image/gif", "IMAGE/PNG", "text/html", "image/png; charset=x"] {
        let fds = set(vec![message("CaptureReply", vec![field("png", 1, Type::Bytes, Some(said))], vec![])]);
        let refused = media_fields(&fds).expect_err(said);
        assert_eq!(refused, vec![DescriptorRefusal::MediaUnknown("minion.v1.CaptureReply.png".into(), said.into())]);
        assert_eq!(refused[0].name(), "media-unknown");
    }
}

/// A descriptor that says no media reads as before: nothing tagged, nothing refused.
#[test]
fn a_descriptor_without_media_is_unchanged() {
    let fds = set(vec![message(
        "CaptureReply",
        vec![field("png", 1, Type::Bytes, None), field("scale", 2, Type::Double, None)],
        vec![],
    )]);
    assert_eq!(media_fields(&fds), Ok(vec![]));
}

/// Option bytes that don't read are malformed, never read as no media.
#[test]
fn unreadable_field_options_are_malformed() {
    let mut raw = field("png", 1, Type::Bytes, None);
    raw.extend(ld(8, &ld(FIELD_OPTIONS_FIELD, &[0xff])));
    let fds = set(vec![message("CaptureReply", vec![raw], vec![])]);
    assert_eq!(
        media_fields(&fds),
        Err(vec![DescriptorRefusal::FieldOptionsMalformed("minion.v1.CaptureReply.png".into())])
    );
}

/// The proto says what the hand-written wire and the extension number say.
#[test]
fn the_field_option_is_the_protos() {
    let proto = crate::protos::OPTIONS;
    assert!(proto.contains("message FieldOptions {"), "FieldOptions is in the proto");
    assert!(proto.contains("string media = 1;"), "media is field 1");
    assert!(proto.contains(&format!("FieldOptions anemnez_field = {FIELD_OPTIONS_FIELD};")), "the extension number");
}

/// The connect check refuses what the media reader refuses, so no bridge admits a tag another refuses.
#[test]
fn check_descriptor_refuses_a_bad_media_tag() {
    let fds = set(vec![message("CaptureReply", vec![field("title", 1, Type::String, Some("image/png"))], vec![])]);
    let refused = crate::descriptor::check_descriptor(&fds).expect_err("refused at connect");
    assert!(refused.contains(&DescriptorRefusal::MediaNotBytes("minion.v1.CaptureReply.title".into())), "{refused:?}");
    let good = set(vec![message("CaptureReply", vec![field("png", 1, Type::Bytes, Some("image/png"))], vec![])]);
    assert_eq!(crate::descriptor::check_descriptor(&good), Ok(vec![]), "a good tag with no offered method");
}
