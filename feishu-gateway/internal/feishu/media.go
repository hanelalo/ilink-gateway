package feishu

import (
	"path/filepath"
	"strings"
)

// extToFileType maps file extensions to feishu's file_type parameter for
// POST /open-apis/im/v1/files. See docs/feishu.md.
var extToFileType = map[string]string{
	".opus": "opus",
	".ogg":  "opus",
	".mp4":  "mp4",
	".pdf":  "pdf",
	".doc":  "doc",
	".docx": "docx",
	".xls":  "xls",
	".xlsx": "xlsx",
	".ppt":  "ppt",
	".pptx": "pptx",
}

// FileTypeForPath returns the feishu file_type for a local file path.
// Unknown extensions fall back to "stream".
func FileTypeForPath(path string) string {
	ext := strings.ToLower(filepath.Ext(path))
	if t, ok := extToFileType[ext]; ok {
		return t
	}
	return "stream"
}

// IsImagePath reports whether the path looks like a raster image.
func IsImagePath(path string) bool {
	switch strings.ToLower(filepath.Ext(path)) {
	case ".jpg", ".jpeg", ".png", ".gif", ".bmp", ".webp":
		return true
	}
	return false
}
