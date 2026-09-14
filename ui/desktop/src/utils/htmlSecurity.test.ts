import { describe, it, expect } from 'vitest';
import { performance } from 'node:perf_hooks';
import { containsHTML, wrapHTMLInCodeBlock } from '../utils/htmlSecurity';

describe('HTML Security Detection', () => {
  describe('containsHTML', () => {
    describe('should detect dangerous HTML tags', () => {
      it('detects script tags', () => {
        expect(containsHTML('<script>alert("xss")</script>')).toBe(true);
        expect(containsHTML('<script src="evil.js"></script>')).toBe(true);
        expect(containsHTML('<script>')).toBe(true);
      });

      it('detects style tags', () => {
        expect(containsHTML('<style>body { display: none; }</style>')).toBe(true);
        expect(containsHTML('<style>')).toBe(true);
      });

      it('detects iframe tags', () => {
        expect(containsHTML('<iframe src="evil.com"></iframe>')).toBe(true);
        expect(containsHTML('<iframe>')).toBe(true);
      });

      it('detects form elements', () => {
        expect(containsHTML('<form action="/submit"></form>')).toBe(true);
        expect(containsHTML('<input type="text" name="password">')).toBe(true);
        expect(containsHTML('<button onclick="evil()">Click</button>')).toBe(true);
      });

      it('detects layout-affecting tags', () => {
        expect(containsHTML('<div class="container">content</div>')).toBe(true);
        expect(containsHTML('<span style="color:red">text</span>')).toBe(true);
        expect(containsHTML('<br/>')).toBe(true);
        expect(containsHTML('<hr>')).toBe(true);
        expect(containsHTML('<img src="image.jpg" alt="test">')).toBe(true);
      });

      it('detects HTML comments', () => {
        expect(containsHTML('<!-- this is a comment -->')).toBe(true);
        expect(containsHTML('<!-- multi\nline\ncomment -->')).toBe(true);
      });
    });

    describe('should NOT detect safe content', () => {
      it('ignores auto-links', () => {
        expect(containsHTML('<https://example.com>')).toBe(false);
        expect(containsHTML('<http://test.org>')).toBe(false);
        expect(containsHTML('<https://block.dev/docs>')).toBe(false);
      });

      it('ignores email addresses', () => {
        expect(containsHTML('<user@example.com>')).toBe(false);
        expect(containsHTML('<admin@block.dev>')).toBe(false);
        expect(containsHTML('<test.email+tag@domain.co.uk>')).toBe(false);
      });

      it('ignores TypeScript generics and placeholders', () => {
        expect(containsHTML('Array<T>')).toBe(false);
        expect(containsHTML('Promise<string>')).toBe(false);
        expect(containsHTML('<project-root>')).toBe(false);
        expect(containsHTML('<filename>')).toBe(false);
        expect(containsHTML('<<not a tag>>')).toBe(false);
      });

      it('ignores content already in code blocks', () => {
        expect(containsHTML('```html\n<div>safe</div>\n```')).toBe(false);
        expect(containsHTML('`<script>safe</script>`')).toBe(false);
        expect(containsHTML('Here is `<br/>` in inline code')).toBe(false);
      });

      it('ignores plain text', () => {
        expect(containsHTML('This is just plain text')).toBe(false);
        expect(containsHTML('No HTML here!')).toBe(false);
        expect(containsHTML('')).toBe(false);
      });

      it('ignores mathematical expressions', () => {
        expect(containsHTML('x < y && y > z')).toBe(false);
        expect(containsHTML('if (a < b && c > d)')).toBe(false);
      });
    });

    describe('edge cases', () => {
      it('handles mixed content correctly', () => {
        // Real HTML mixed with safe content
        expect(containsHTML('Visit <https://example.com> and <div>click here</div>')).toBe(true);

        // Only safe content
        expect(containsHTML('Email <user@test.com> about <project-root> setup')).toBe(false);
      });

      it('handles malformed HTML', () => {
        expect(containsHTML('<div unclosed')).toBe(false); // This doesn't match our regex pattern
        expect(containsHTML('<>')).toBe(false);
        expect(containsHTML('< div >')).toBe(false);
      });

      it('rejects unterminated comment prefixes without blocking the renderer', () => {
        const maliciousContent = '<!--'.repeat(64_000);
        const startedAt = performance.now();

        expect(containsHTML(maliciousContent)).toBe(false);

        expect(performance.now() - startedAt).toBeLessThan(750);
      }, 10_000);
    });
  });

  describe('wrapHTMLInCodeBlock', () => {
    describe('should wrap dangerous HTML', () => {
      it('wraps single line HTML', () => {
        const input = '<script>alert("xss")</script>';
        const expected = '```html\n<script>alert("xss")</script>\n```';
        expect(wrapHTMLInCodeBlock(input)).toBe(expected);
      });

      it('wraps HTML comments', () => {
        const input = '<!-- malicious comment -->';
        const expected = '```html\n<!-- malicious comment -->\n```';
        expect(wrapHTMLInCodeBlock(input)).toBe(expected);
      });

      it('wraps mixed content selectively', () => {
        const input = `Normal text
<div>This should be wrapped</div>
More normal text`;

        const expected = `Normal text
\`\`\`html
<div>This should be wrapped</div>
\`\`\`
More normal text`;

        expect(wrapHTMLInCodeBlock(input)).toBe(expected);
      });
    });

    describe('should preserve safe content', () => {
      it('preserves auto-links', () => {
        const input = 'Visit <https://example.com> for more info';
        expect(wrapHTMLInCodeBlock(input)).toBe(input);
      });

      it('preserves email addresses', () => {
        const input = 'Contact <admin@example.com> for help';
        expect(wrapHTMLInCodeBlock(input)).toBe(input);
      });

      it('preserves TypeScript generics', () => {
        const input = 'const arr: Array<string> = []';
        expect(wrapHTMLInCodeBlock(input)).toBe(input);
      });

      it('preserves existing code blocks', () => {
        const input = `# Title

\`\`\`javascript
const x = "<div>this is safe</div>";
\`\`\`

Normal text`;

        expect(wrapHTMLInCodeBlock(input)).toBe(input);
      });

      it('preserves inline code', () => {
        const input = 'Use `<br/>` for line breaks';
        expect(wrapHTMLInCodeBlock(input)).toBe(input);
      });
    });

    describe('complex scenarios', () => {
      it('handles multiple HTML lines correctly', () => {
        const input = `# Test Message

Normal paragraph

<div>First HTML line</div>
<span>Second HTML line</span>

More normal text`;

        const expected = `# Test Message

Normal paragraph

\`\`\`html
<div>First HTML line</div>
\`\`\`
\`\`\`html
<span>Second HTML line</span>
\`\`\`

More normal text`;

        expect(wrapHTMLInCodeBlock(input)).toBe(expected);
      });

      it('respects existing code block boundaries', () => {
        const input = `Before code block

\`\`\`html
<div>This is already safe</div>
<script>This is also safe in here</script>
\`\`\`

<div>This should be wrapped</div>`;

        const expected = `Before code block

\`\`\`html
<div>This is already safe</div>
<script>This is also safe in here</script>
\`\`\`

\`\`\`html
<div>This should be wrapped</div>
\`\`\``;

        expect(wrapHTMLInCodeBlock(input)).toBe(expected);
      });

      it('handles the test suite scenarios', () => {
        // Test Message 1: One-liners
        const test1 = `<https://example.com>
<user@example.com>
\`<T>\``;
        expect(wrapHTMLInCodeBlock(test1)).toBe(test1);

        // Test Message 2: Mixed content - our function wraps the entire line, not just the HTML part
        const test2 = `Here's a link <https://example.com> and HTML <div>content</div>`;
        const expected2 = `\`\`\`html
Here's a link <https://example.com> and HTML <div>content</div>
\`\`\``;
        expect(wrapHTMLInCodeBlock(test2)).toBe(expected2);

        // Test Message 7: Comment-only
        const test7 = `<!-- top-level html comment -->`;
        const expected7 = `\`\`\`html
<!-- top-level html comment -->
\`\`\``;
        expect(wrapHTMLInCodeBlock(test7)).toBe(expected7);
      });
    });

    describe('edge cases', () => {
      it('handles empty input', () => {
        expect(wrapHTMLInCodeBlock('')).toBe('');
      });

      it('handles only whitespace', () => {
        const input = '   \n  \n  ';
        expect(wrapHTMLInCodeBlock(input)).toBe(input);
      });

      it('handles nested code block scenarios', () => {
        const input = `\`\`\`
<div>safe in code block</div>
\`\`\`
<div>unsafe outside</div>
\`\`\`
<span>also safe in code block</span>
\`\`\``;

        const expected = `\`\`\`
<div>safe in code block</div>
\`\`\`
\`\`\`html
<div>unsafe outside</div>
\`\`\`
\`\`\`
<span>also safe in code block</span>
\`\`\``;

        expect(wrapHTMLInCodeBlock(input)).toBe(expected);
      });
    });
  });
});
