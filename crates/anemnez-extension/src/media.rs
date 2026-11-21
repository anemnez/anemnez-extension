//! What a `bytes` field holds, when its descriptor says (CONTRACT.md `#a-bytes-field-may-say-it-is-an-image`): the
//! field option `anemnez_field.media`, so a bridge can show an image as an image (Agora's MCP face answers it as
//! image content, I m/20935) and never guess from the bytes. One reader for every bridge and product (rule 2).
//!
//! Like `anemnez_method`, the option is a protobuf extension that `prost_types` drops on decode, so it is read from
//! each field's raw option bytes. The set of media is closed: an untagged `bytes` field stays bytes, a tag on any
//! other kind of field or naming any other media refuses the descriptor by name.

use prost::Message;
use prost_types::field_descriptor_proto::Type;
use prost_types::{DescriptorProto, FileDescriptorSet};

use crate::descriptor::{DescriptorRefusal, extension, repeated};

/// The extension field number of `anemnez_field` on `google.protobuf.FieldOptions`.
pub const FIELD_OPTIONS_FIELD: u32 = 51703;

/// What a tagged `bytes` field holds. Closed: a media not named here refuses the descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Media {
    /// `image/png`.
    Png,
    /// `image/jpeg`.
    Jpeg,
}

impl Media {
    /// Every media the option may name.
    pub const ALL: [Media; 2] = [Media::Png, Media::Jpeg];

    /// The media type, as the option spells it and a bridge answers it.
    #[must_use]
    pub fn mime(self) -> &'static str {
        match self {
            Media::Png => "image/png",
            Media::Jpeg => "image/jpeg",
        }
    }

    /// The media `mime` names exactly, or `None`.
    #[must_use]
    pub fn from_mime(mime: &str) -> Option<Media> {
        Media::ALL.into_iter().find(|m| m.mime() == mime)
    }

    /// Whether a bridge shows it as an image.
    #[must_use]
    pub fn is_image(self) -> bool {
        matches!(self, Media::Png | Media::Jpeg)
    }
}

/// One tagged field: the message it is in (`.pkg.Message`, nested names joined by `.`), its name and its media.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct MediaField {
    pub message: String,
    pub field: String,
    pub media: Media,
}

/// Every field of every message in the set that says its media, in descriptor order, with every refusal: a tag on
/// a field that isn't `bytes` (`media-not-bytes`), one naming a media not in [`Media::ALL`] (`media-unknown`), and
/// option bytes that don't read (`field-options-malformed`).
///
/// # Errors
/// Every [`DescriptorRefusal`] found, in descriptor order.
pub fn media_fields(file_descriptor_set: &[u8]) -> Result<Vec<MediaField>, Vec<DescriptorRefusal>> {
    let set = FileDescriptorSet::decode(file_descriptor_set)
        .map_err(|e| vec![DescriptorRefusal::Malformed(e.to_string())])?;
    let raw_files = repeated(file_descriptor_set, 1).map_err(|why| vec![DescriptorRefusal::Malformed(why.into())])?;
    if raw_files.len() != set.file.len() {
        return Err(vec![DescriptorRefusal::Malformed(format!(
            "{} files decoded, {} read raw",
            set.file.len(),
            raw_files.len()
        ))]);
    }
    let mut found = Vec::new();
    let mut refused = Vec::new();
    for (file, raw_file) in set.file.iter().zip(raw_files) {
        let prefix = if file.package().is_empty() { String::new() } else { format!(".{}", file.package()) };
        let raw_messages = repeated(raw_file, 4).map_err(|why| vec![malformed(file.name(), why)])?;
        if raw_messages.len() != file.message_type.len() {
            return Err(vec![malformed(file.name(), "the messages don't read raw")]);
        }
        for (message, raw) in file.message_type.iter().zip(raw_messages) {
            walk(&prefix, message, raw, &mut found, &mut refused).map_err(|why| vec![malformed(file.name(), why)])?;
        }
    }
    if refused.is_empty() { Ok(found) } else { Err(refused) }
}

/// The tagged top-level fields of message `type_name` (`.pkg.Message`), by field name: what a bridge reads to show
/// one reply's images as images.
///
/// # Errors
/// As [`media_fields`].
pub fn media_of(file_descriptor_set: &[u8], type_name: &str) -> Result<Vec<(String, Media)>, Vec<DescriptorRefusal>> {
    Ok(media_fields(file_descriptor_set)?
        .into_iter()
        .filter(|f| f.message == type_name)
        .map(|f| (f.field, f.media))
        .collect())
}

fn malformed(file: &str, why: &str) -> DescriptorRefusal {
    DescriptorRefusal::Malformed(format!("{file}: {why}"))
}

fn walk(
    parent: &str,
    message: &DescriptorProto,
    raw: &[u8],
    found: &mut Vec<MediaField>,
    refused: &mut Vec<DescriptorRefusal>,
) -> Result<(), &'static str> {
    let name = format!("{parent}.{}", message.name());
    let raw_fields = repeated(raw, 2)?;
    if raw_fields.len() != message.field.len() {
        return Err("a message's fields don't read raw");
    }
    for (field, raw_field) in message.field.iter().zip(raw_fields) {
        let path = format!("{}.{}", name.trim_start_matches('.'), field.name());
        let bytes = match extension(raw_field, 8, FIELD_OPTIONS_FIELD) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => continue,
            Err(_) => {
                refused.push(DescriptorRefusal::FieldOptionsMalformed(path));
                continue;
            }
        };
        let Ok(options) = wire::FieldOptions::decode(bytes.as_slice()) else {
            refused.push(DescriptorRefusal::FieldOptionsMalformed(path));
            continue;
        };
        if options.media.is_empty() {
            continue;
        }
        let Some(media) = Media::from_mime(&options.media) else {
            refused.push(DescriptorRefusal::MediaUnknown(path, options.media));
            continue;
        };
        if field.r#type() != Type::Bytes {
            refused.push(DescriptorRefusal::MediaNotBytes(path));
            continue;
        }
        found.push(MediaField { message: name.clone(), field: field.name().to_owned(), media });
    }
    let raw_nested = repeated(raw, 3)?;
    if raw_nested.len() != message.nested_type.len() {
        return Err("a message's nested types don't read raw");
    }
    for (nested, raw) in message.nested_type.iter().zip(raw_nested) {
        walk(&name, nested, raw, found, refused)?;
    }
    Ok(())
}

/// `anemnez.extension.v1.FieldOptions` as it travels, written by hand as `descriptor`'s `wire` is; the serve
/// crate's parity test holds it to the proto's generated code.
pub(crate) mod wire {
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct FieldOptions {
        #[prost(string, tag = "1")]
        pub media: String,
    }
}

#[cfg(test)]
#[path = "media_tests.rs"]
mod tests;
