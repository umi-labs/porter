use porter_engine::project::Project;
use std::fs;

fn project() -> (tempfile::TempDir, Project) {
    let d = tempfile::tempdir().unwrap();
    let p = Project::init(d.path(), "t", "wordpress").unwrap();
    (d, p)
}

#[test]
fn same_bytes_dedupe() {
    let (_d, p) = project();
    let s = p.store();
    let a = s
        .put_media_bytes(b"img", "https://x.com/a-300x200.jpg", "a.jpg")
        .unwrap();
    let b = s
        .put_media_bytes(b"img", "https://x.com/a.jpg", "a.jpg")
        .unwrap();
    assert!(a.is_new);
    assert!(!b.is_new);
    assert_eq!(a.id, b.id);
    assert_eq!(
        s.media_by_url("https://x.com/a-300x200.jpg").unwrap(),
        Some(a.id)
    );
    assert_eq!(s.media_by_url("https://x.com/a.jpg").unwrap(), Some(a.id));
    assert_eq!(s.media_path(a.id).unwrap(), a.path);
    assert!(a.path.starts_with(p.media_dir()));
}

#[test]
fn case_collision_kept_apart() {
    let (_d, p) = project();
    let s = p.store();
    let a = s
        .put_media_bytes(b"one", "https://x.com/Photo.jpg", "Photo.jpg")
        .unwrap();
    let b = s
        .put_media_bytes(b"two", "https://x.com/photo.jpg", "photo.jpg")
        .unwrap();
    assert_ne!(a.id, b.id);
    assert_eq!(fs::read(&a.path).unwrap(), b"one");
    assert_eq!(fs::read(&b.path).unwrap(), b"two");
    assert_eq!(
        s.media_by_url("https://x.com/Photo.jpg").unwrap(),
        Some(a.id)
    );
    assert_eq!(
        s.media_by_url("https://x.com/photo.jpg").unwrap(),
        Some(b.id)
    );
}

#[test]
fn case_insensitive_fallback_when_unique() {
    let (_d, p) = project();
    let s = p.store();
    let a = s
        .put_media_bytes(b"one", "https://x.com/Photo.jpg", "Photo.jpg")
        .unwrap();
    assert_eq!(
        s.media_by_url("https://x.com/photo.jpg").unwrap(),
        Some(a.id)
    );
}

#[test]
fn ambiguous_case_lookup_is_none() {
    let (_d, p) = project();
    let s = p.store();
    s.put_media_bytes(b"one", "https://x.com/Photo.jpg", "Photo.jpg")
        .unwrap();
    s.put_media_bytes(b"two", "https://x.com/photo.jpg", "photo.jpg")
        .unwrap();
    assert_eq!(s.media_by_url("https://x.com/PHOTO.JPG").unwrap(), None);
}

#[test]
fn replaced_variant_reported() {
    let (_d, p) = project();
    let s = p.store();
    let a = s
        .put_media_bytes(b"one", "https://x.com/a.jpg", "a.jpg")
        .unwrap();
    assert_eq!(a.replaced, None);
    let b = s
        .put_media_bytes(b"two", "https://x.com/a.jpg", "a.jpg")
        .unwrap();
    assert_eq!(b.replaced, Some(a.id));
    assert_eq!(s.media_by_url("https://x.com/a.jpg").unwrap(), Some(b.id));
}

#[test]
fn variant_lookup_normalises() {
    let (_d, p) = project();
    let s = p.store();
    let a = s
        .put_media_bytes(b"img", "https://x.com/a.jpg", "a.jpg")
        .unwrap();
    assert_eq!(
        s.media_by_url("http://www.x.com/a.jpg").unwrap(),
        Some(a.id)
    );
    assert_eq!(s.media_by_url("https://x.com/none.jpg").unwrap(), None);
}

/// A stored hash that is short, or not hex, is an error: never a slice panic or a path
/// that leaves the media directory.
#[test]
fn media_path_rejects_bad_hashes() {
    use porter_model::{ItemKind, Media, SourceRef};
    let (_d, p) = project();
    let s = p.store();
    for (n, hash) in ["abc", "", "../../../../etc/passwd", &"G".repeat(64)]
        .into_iter()
        .enumerate()
    {
        let src = SourceRef {
            connector: "media".into(),
            source_id: n.to_string(),
            url: None,
        };
        let id = porter_model::ItemId::new(ItemKind::Media, &src);
        let m = Media {
            id,
            hash: hash.into(),
            variants: vec![],
            filename: "x".into(),
            mime: "image/png".into(),
            width: None,
            height: None,
            alt: None,
            caption: None,
            title: None,
            credit: None,
            folder: None,
            focal_x: None,
            focal_y: None,
            usages: vec![],
        };
        s.put_item(ItemKind::Media, id, &src, &m).unwrap();
        let e = s.media_path(id).unwrap_err().to_string();
        assert!(e.contains("invalid media hash"), "{hash:?}: {e}");
    }
}
