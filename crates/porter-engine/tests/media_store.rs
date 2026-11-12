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
