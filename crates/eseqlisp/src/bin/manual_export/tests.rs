use super::*;

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("eseq-manual-export-{}-{nonce}", std::process::id()));
        fs::create_dir_all(root.join("source/images")).unwrap();
        Self(root)
    }

    fn source(&self) -> PathBuf { self.0.join("source") }
    fn output(&self) -> PathBuf { self.0.join("site/manual") }
    fn write(&self, name: &str, text: &str) { fs::write(self.source().join(name), text).unwrap(); }
    fn read(&self, name: &str) -> String { fs::read_to_string(self.output().join(name)).unwrap() }
}

impl Drop for Fixture {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}

#[test]
fn exports_shared_ast_images_navigation_and_inert_actions() {
    let fixture = Fixture::new();
    fixture.write("index.md", "# Manual & <intro>\n\n- [Second](b) — start here\n- [First](a) — then here\n");
    fixture.write("b.md", r#"# Second

**Bold** *emphasis* `code` [other](a) [web](https://example.com/?a=1&b=2) [app](action:dangerous-form)

![Caption "quoted" & <safe>](<images/a figure.png>)

1. First item
2. Second item

```lisp
<script>do_not_execute()</script>
```

## Same heading

## Same heading
"#);
    fixture.write("a.md", "# First\n\nA paragraph.\n");
    let asset = fixture.source().join("images/a figure.png");
    image::RgbaImage::from_pixel(40, 20, image::Rgba([1, 2, 3, 255])).save(&asset).unwrap();
    assert_eq!(export(&fixture.source(), &fixture.output()).unwrap(), (3, 1));
    let html = fixture.read("b.html");
    for expected in [
        "<strong>Bold</strong>", "<em>emphasis</em>", "<code>code</code>",
        "<a href=\"a.html\">other</a>", "https://example.com/?a=1&amp;b=2",
        "<span class=\"app-action\">app</span>", "&lt;script&gt;do_not_execute()&lt;/script&gt;",
        "images/a%20figure.png", "width=\"40\" height=\"20\"", "<ol>\n<li>First item</li>",
        "rel=\"prev\" href=\"index.html\"", "rel=\"next\" href=\"a.html\"",
        "id=\"same-heading\"", "id=\"same-heading-2\"", "aria-current=\"page\"",
        "Caption &quot;quoted&quot; &amp; &lt;safe&gt;",
    ] { assert!(html.contains(expected), "missing {expected}"); }
    assert!(!html.contains("dangerous-form"));
    assert!(!html.contains("<script>"));
    let index = fixture.read("index.html");
    assert!(index.contains("Manual &amp; &lt;intro&gt;"));
    assert!(index.find("href=\"b.html\"").unwrap() < index.find("href=\"a.html\"").unwrap());
    assert_eq!(fs::read(asset).unwrap(), fs::read(fixture.output().join("images/a figure.png")).unwrap());
    assert!(fixture.output().join("manual.css").is_file());
}

#[test]
fn validation_leaves_previous_export_intact() {
    let fixture = Fixture::new();
    fixture.write("index.md", "# Good page\n\nOriginal text.");
    export(&fixture.source(), &fixture.output()).unwrap();
    let original = fixture.read("index.html");
    for invalid in [
        "# Page\n\n[missing](absent)",
        "# Page\n\n[unsafe](javascript:alert)",
        "# Page\n\n![missing](images/missing.png)",
        "# Page\n\n![escape](images/../../escape.png)",
        "# Page\n\n- [Missing](absent)",
        "No heading",
    ] {
        fixture.write("index.md", invalid);
        assert!(export(&fixture.source(), &fixture.output()).is_err(), "must reject {invalid}");
        assert_eq!(fixture.read("index.html"), original);
    }
}

#[test]
fn reruns_remove_only_stale_generated_files_and_reject_unowned_collisions() {
    let fixture = Fixture::new();
    fixture.write("index.md", "# Manual\n\n- [Old](old)");
    fixture.write("old.md", "# Old page");
    export(&fixture.source(), &fixture.output()).unwrap();
    fs::write(fixture.output().join("custom.txt"), "keep me").unwrap();
    fixture.write("index.md", "# Manual\n\nUpdated.");
    fs::remove_file(fixture.source().join("old.md")).unwrap();
    export(&fixture.source(), &fixture.output()).unwrap();
    assert!(!fixture.output().join("old.html").exists());
    assert_eq!(fixture.read("custom.txt"), "keep me");
    fs::write(fixture.output().join("new.html"), "authored separately").unwrap();
    fixture.write("new.md", "# New page");
    assert!(export(&fixture.source(), &fixture.output()).unwrap_err().contains("refusing to overwrite"));
    assert_eq!(fixture.read("new.html"), "authored separately");
}
