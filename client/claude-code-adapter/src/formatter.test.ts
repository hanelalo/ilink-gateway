import { describe, it, expect } from 'vitest';
import { stripMarkdown } from './formatter.js';

describe('stripMarkdown', () => {
  it('should handle empty string', () => {
    expect(stripMarkdown('')).toBe('');
  });

  it('should pass through plain text unchanged', () => {
    expect(stripMarkdown('hello world')).toBe('hello world');
  });

  it('should strip bold markers', () => {
    expect(stripMarkdown('This is **bold** text')).toBe('This is bold text');
  });

  it('should strip italic markers', () => {
    expect(stripMarkdown('This is *italic* text')).toBe('This is italic text');
  });

  it('should strip inline code markers', () => {
    expect(stripMarkdown('Use `code` here')).toBe('Use code here');
  });

  it('should strip code block markers and keep content', () => {
    const input = '```\nconst x = 1;\n```';
    expect(stripMarkdown(input)).toBe('\nconst x = 1;\n');
  });

  it('should handle code block with language identifier', () => {
    const input = '```typescript\nconst x: number = 1;\n```';
    expect(stripMarkdown(input)).toBe('\nconst x: number = 1;\n');
  });

  it('should convert markdown headings to 【】 format', () => {
    expect(stripMarkdown('# Title')).toBe('【Title】');
    expect(stripMarkdown('## Subtitle')).toBe('【Subtitle】');
  });

  it('should convert headings of different levels', () => {
    expect(stripMarkdown('###### Tiny')).toBe('【Tiny】');
  });

  it('should convert list items with dash', () => {
    expect(stripMarkdown('- item 1\n- item 2')).toBe('· item 1\n· item 2');
  });

  it('should convert list items with asterisk', () => {
    expect(stripMarkdown('* item 1\n* item 2')).toBe('· item 1\n· item 2');
  });

  it('should handle nested list items', () => {
    expect(stripMarkdown('  - nested item')).toBe('  · nested item');
  });

  it('should convert markdown links', () => {
    expect(stripMarkdown('[text](http://example.com)')).toBe('text (http://example.com)');
  });

  it('should handle multiple links', () => {
    const input = 'See [page1](url1) and [page2](url2)';
    expect(stripMarkdown(input)).toBe('See page1 (url1) and page2 (url2)');
  });

  it('should remove horizontal separators', () => {
    expect(stripMarkdown('text\n---\nmore')).toBe('text\n\nmore');
  });

  it('should handle nested bold and italic (***)', () => {
    expect(stripMarkdown('***bold and italic***')).toBe('bold and italic');
  });

  it('should handle multiple markdown patterns combined', () => {
    const input = [
      '# Title',
      '',
      'This is **bold** and *italic* with `code`.',
      '',
      '- **important** item',
      '- another item',
      '',
      'See [docs](https://example.com) for more.',
      '',
      '---',
      '',
      '```',
      'const x = 1;',
      '```',
    ].join('\n');
    const expected = [
      '【Title】',
      '',
      'This is bold and italic with code.',
      '',
      '· important item',
      '· another item',
      '',
      'See docs (https://example.com) for more.',
      '',
      '',
      '',
      '',
      'const x = 1;',
      '',
    ].join('\n');
    expect(stripMarkdown(input)).toBe(expected);
  });

  it('should preserve newlines', () => {
    expect(stripMarkdown('line1\n\nline2')).toBe('line1\n\nline2');
  });

  it('should handle bold with adjacent text', () => {
    expect(stripMarkdown('before**bold**after')).toBe('beforeboldafter');
  });

  it('should handle italic with adjacent text', () => {
    expect(stripMarkdown('before*italic*after')).toBe('beforeitalicafter');
  });
});
