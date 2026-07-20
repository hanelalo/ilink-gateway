package feishu

import "testing"

func TestFileTypeForPath(t *testing.T) {
	cases := map[string]string{
		"song.opus":   "opus",
		"audio.OGG":   "opus",
		"clip.mp4":    "mp4",
		"doc.pdf":     "pdf",
		"report.docx": "docx",
		"sheet.xlsx":  "xlsx",
		"slides.PPTX": "pptx",
		"unknown.xyz": "stream",
		"noext":       "stream",
	}
	for path, want := range cases {
		if got := FileTypeForPath(path); got != want {
			t.Errorf("FileTypeForPath(%q) = %q, want %q", path, got, want)
		}
	}
}

func TestIsImagePath(t *testing.T) {
	images := []string{"a.jpg", "b.JPEG", "c.png", "d.gif", "e.webp", "f.bmp"}
	for _, p := range images {
		if !IsImagePath(p) {
			t.Errorf("expected image: %q", p)
		}
	}
	nonImages := []string{"a.pdf", "b.mp4", "c.doc", "noext"}
	for _, p := range nonImages {
		if IsImagePath(p) {
			t.Errorf("expected non-image: %q", p)
		}
	}
}
