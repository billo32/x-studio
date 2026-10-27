use std::io::{Cursor, Read, Write};

use base64::Engine;
use image::{Rgb, RgbImage};
use x4_books::epub::{self, Edits};
use x4_books::imaging::{self, Fit};
use x4_books::{fb2, read_book};
use zip::write::SimpleFileOptions;

fn png(w: u32, h: u32, color: [u8; 3]) -> Vec<u8> {
    imaging::encode(&RgbImage::from_pixel(w, h, Rgb(color)), "png").unwrap()
}

fn make_epub(with_cover: bool) -> Vec<u8> {
    let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let stored = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    z.start_file("mimetype", stored).unwrap();
    z.write_all(b"application/epub+zip").unwrap();
    z.start_file("META-INF/container.xml", SimpleFileOptions::default())
        .unwrap();
    z.write_all(br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#).unwrap();
    let cover_meta = if with_cover {
        r#"<meta name="cover" content="img-cover"/>"#
    } else {
        ""
    };
    let cover_item = if with_cover {
        r#"<item id="img-cover" href="Images/cover%20art.jpg" media-type="image/jpeg"/>"#
    } else {
        ""
    };
    let opf = format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="id">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
    <dc:title>Преступление &amp; наказание</dc:title>
    <dc:creator opf:file-as="Достоевский, Фёдор" opf:role="aut">Фёдор Достоевский</dc:creator>
    <dc:creator opf:role="trl">Someone Else</dc:creator>
    <dc:language>ru</dc:language>
    <meta name="calibre:series" content="Классика"/>
    {cover_meta}
  </metadata>
  <manifest>
    <item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
    <item id="c1" href="Text/ch1.xhtml" media-type="application/xhtml+xml"/>
    <item id="c2" href="Text/ch2.xhtml" media-type="application/xhtml+xml"/>
    {cover_item}
    <item id="img2" href="Images/fig.png" media-type="image/png"/>
  </manifest>
  <spine toc="ncx"><itemref idref="c1"/><itemref idref="c2"/></spine>
</package>"#
    );
    z.start_file("OEBPS/content.opf", SimpleFileOptions::default())
        .unwrap();
    z.write_all(opf.as_bytes()).unwrap();
    z.start_file("OEBPS/toc.ncx", SimpleFileOptions::default())
        .unwrap();
    z.write_all(
        b"<ncx><navMap><navPoint id='a'/><navPoint id='b'/><navPoint id='c'/></navMap></ncx>",
    )
    .unwrap();
    let para = "Был июль. ".repeat(200); // 2000 chars
    for (i, name) in ["ch1", "ch2"].iter().enumerate() {
        z.start_file(
            format!("OEBPS/Text/{name}.xhtml"),
            SimpleFileOptions::default(),
        )
        .unwrap();
        z.write_all(format!("<html><head><title>T{i}</title><style>p{{}}</style></head><body><p>{para}</p></body></html>").as_bytes()).unwrap();
    }
    if with_cover {
        z.start_file("OEBPS/Images/cover art.jpg", stored).unwrap();
        let jpg = imaging::encode(&RgbImage::from_pixel(60, 90, Rgb([10, 20, 30])), "jpg").unwrap();
        z.write_all(&jpg).unwrap();
    }
    z.start_file("OEBPS/Images/fig.png", stored).unwrap();
    z.write_all(&png(4, 4, [1, 2, 3])).unwrap();
    z.finish().unwrap().into_inner()
}

#[test]
fn reads_epub_metadata_stats_and_cover() {
    let (info, cover) = read_book("x.epub", &make_epub(true)).unwrap();
    assert_eq!(info.title, "Преступление & наказание");
    assert_eq!(info.authors, vec!["Фёдор Достоевский", "Someone Else"]);
    assert_eq!(info.language.as_deref(), Some("ru"));
    assert_eq!(info.series.as_deref(), Some("Классика"));
    assert_eq!(info.chapters, 3, "from the NCX");
    assert_eq!(info.images, 2);
    assert!((3990..=4010).contains(&info.chars), "{}", info.chars);
    assert_eq!(info.pages, 3);
    let cover = cover.expect("cover");
    assert_eq!(cover.path.as_deref(), Some("OEBPS/Images/cover art.jpg"));
    assert_eq!(imaging::preview(&cover.bytes, 50, 50).unwrap().width, 60);
}

#[test]
fn rewrites_metadata_and_replaces_cover() {
    let baked = imaging::bake(
        &png(300, 300, [200, 0, 0]),
        480,
        800,
        Fit::Contain,
        [255, 255, 255],
    )
    .unwrap();
    let edits = Edits {
        title: Some("Новое <название>".into()),
        author: Some("Ф. М. Достоевский".into()),
        language: None,
        series: Some(String::new()),
        cover: Some((imaging::encode(&baked, "jpg").unwrap(), "jpg")),
    };
    let out = epub::rewrite(&make_epub(true), &edits).unwrap();

    // mimetype first and stored
    let mut z = zip::ZipArchive::new(Cursor::new(&out[..])).unwrap();
    {
        let first = z.by_index(0).unwrap();
        assert_eq!(first.name(), "mimetype");
        assert_eq!(first.compression(), zip::CompressionMethod::Stored);
    }
    let mut opf = String::new();
    z.by_name("OEBPS/content.opf")
        .unwrap()
        .read_to_string(&mut opf)
        .unwrap();
    assert!(!opf.contains("file-as"), "stale file-as dropped");
    assert!(!opf.contains("calibre:series"));

    let (info, cover) = read_book("x.epub", &out).unwrap();
    assert_eq!(info.title, "Новое <название>");
    assert_eq!(info.authors, vec!["Ф. М. Достоевский"]);
    assert_eq!(info.language.as_deref(), Some("ru"));
    assert_eq!(info.series, None);
    let p = imaging::preview(&cover.unwrap().bytes, 10, 10).unwrap();
    assert_eq!((p.width, p.height), (480, 800));
}

#[test]
fn adds_cover_when_missing() {
    let (_, none) = read_book("x.epub", &make_epub(false)).unwrap();
    assert!(none.is_none());
    let edits = Edits {
        cover: Some((png(48, 80, [0, 0, 0]), "png")),
        ..Default::default()
    };
    let out = epub::rewrite(&make_epub(false), &edits).unwrap();
    let (_, cover) = read_book("x.epub", &out).unwrap();
    let cover = cover.expect("added cover");
    assert_eq!(cover.path.as_deref(), Some("OEBPS/xstudio-cover.png"));
}

#[test]
fn reads_fb2_in_cp1251_with_cover() {
    let cover = png(30, 45, [9, 9, 9]);
    let b64 = base64::engine::general_purpose::STANDARD.encode(&cover);
    let xml = format!(
        r##"<?xml version="1.0" encoding="windows-1251"?>
<FictionBook xmlns="http://www.gribuser.ru/xml/fictionbook/2.0" xmlns:l="http://www.w3.org/1999/xlink">
<description><title-info><author><first-name>Николай</first-name><last-name>Гоголь</last-name></author>
<book-title>Мёртвые души</book-title><coverpage><image l:href="#cover.png"/></coverpage><lang>ru</lang>
<sequence name="Поэмы" number="1"/></title-info>
<document-info><author><nickname>maker</nickname><first-name>Не</first-name><last-name>Автор</last-name></author></document-info></description>
<body><section><title><p>Глава первая</p></title><p>{}</p></section><section><p>Текст</p></section></body>
<binary id="cover.png" content-type="image/png">{b64}</binary>
</FictionBook>"##,
        "В ворота гостиницы ".repeat(100)
    );
    let (bytes, _, _) = encoding_rs::WINDOWS_1251.encode(&xml);
    let (info, cover_out) = fb2::read(&bytes).unwrap();
    assert_eq!(info.title, "Мёртвые души");
    assert_eq!(info.authors, vec!["Николай Гоголь"]);
    assert_eq!(info.language.as_deref(), Some("ru"));
    assert_eq!(info.series.as_deref(), Some("Поэмы"));
    assert_eq!(info.chapters, 2);
    assert!(!info.editable);
    assert!(info.chars > 1900);
    assert_eq!(cover_out.unwrap().bytes, cover);
}
