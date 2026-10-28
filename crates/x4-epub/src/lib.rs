//! EPUB clean-up for books downloaded from online stores: metadata in the OPF, a cover page,
//! XHTML repaired, embedded fonts dropped. Offered to plugins as `xstudio.epub.prepare`.

// Vendored from yaebook; kept in upstream formatting (see epub/rustfmt.toml).
#[rustfmt::skip]
pub mod epub;
pub mod meta;

pub use epub::{Error, prepare_epub};
pub use meta::{BookMetadata, SeriesInfo, file_name};

/// Cleans `data` (an EPUB) with `metadata` and an optional cover image.
pub fn prepare(
    data: &[u8],
    metadata: &BookMetadata,
    cover: Option<&[u8]>,
) -> Result<Vec<u8>, Error> {
    if !data.starts_with(b"PK") {
        return Err(Error::InvalidArchive("not an EPUB (ZIP) file".into()));
    }
    prepare_epub(data, metadata, cover, |_| {})
}
