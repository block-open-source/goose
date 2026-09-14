import { describe, it, expect, vi } from 'vitest';
import { fireEvent, render, type RenderOptions } from '@testing-library/react';
import { screen, waitFor } from '@testing-library/dom';
import MarkdownContent from './MarkdownContent';
import { IntlTestWrapper } from '../i18n/test-utils';

const renderWithIntl = (ui: React.ReactElement, options?: RenderOptions) =>
  render(ui, { wrapper: IntlTestWrapper, ...options });

// Mock the icons to avoid import issues
vi.mock('./icons', () => ({
  Check: () => <div data-testid="check-icon">✓</div>,
  Copy: () => <div data-testid="copy-icon">📋</div>,
}));

describe('MarkdownContent', () => {
  describe('HTML Security Integration', () => {
    it('renders safe markdown content normally', async () => {
      const content = `# Test Title

Visit <https://example.com> for more info.

Contact <admin@example.com> for support.

Use \`Array<T>\` for generics.`;

      renderWithIntl(<MarkdownContent content={content} />);

      await waitFor(() => {
        expect(screen.getByText('Test Title')).toBeInTheDocument();
        expect(screen.getByText(/Visit/)).toBeInTheDocument();
        expect(screen.getByText(/for more info/)).toBeInTheDocument();
        expect(screen.getByText(/Contact/)).toBeInTheDocument();
        expect(screen.getByText(/for support/)).toBeInTheDocument();
      });

      // Should not create extra code blocks for safe content
      const codeBlocks = screen.queryAllByText(/```html/);
      expect(codeBlocks).toHaveLength(0);
    });

    it('wraps dangerous HTML in code blocks', async () => {
      const content = `# Security Test

This is safe text.

<script>alert('xss')</script>

More safe text.`;

      renderWithIntl(<MarkdownContent content={content} />);

      await waitFor(() => {
        expect(screen.getByText('Security Test')).toBeInTheDocument();
        expect(screen.getByText('This is safe text.')).toBeInTheDocument();
        expect(screen.getByText('More safe text.')).toBeInTheDocument();
      });

      // The script tag should be in a code block, not executed
      const scriptElements = document.querySelectorAll('script');
      expect(scriptElements).toHaveLength(0); // No actual script tags should be created

      // Should find the script content in a code block (text may be split across spans)
      await waitFor(() => {
        expect(screen.getByText(/alert/)).toBeInTheDocument();
        expect(screen.getByText(/xss/)).toBeInTheDocument();
      });
    });

    it('handles HTML comments securely', async () => {
      const content = `# Comment Test

<!-- This is a malicious comment -->

Normal text continues.`;

      renderWithIntl(<MarkdownContent content={content} />);

      await waitFor(() => {
        expect(screen.getByText('Comment Test')).toBeInTheDocument();
        expect(screen.getByText('Normal text continues.')).toBeInTheDocument();
      });

      // Comment should be in a code block
      await waitFor(() => {
        expect(screen.getByText(/This is a malicious comment/)).toBeInTheDocument();
      });
    });

    it('preserves existing code blocks', async () => {
      const content = `# Code Block Test

\`\`\`javascript
const html = "<div>This is safe in a code block</div>";
console.log(html);
\`\`\`

<div>This should be wrapped</div>`;

      renderWithIntl(<MarkdownContent content={content} />);

      await waitFor(() => {
        expect(screen.getByText('Code Block Test')).toBeInTheDocument();
      });

      // Should preserve the original JavaScript code block (text may be split)
      await waitFor(() => {
        expect(screen.getByText(/const/)).toBeInTheDocument();
        expect(screen.getAllByText(/html/)).toHaveLength(2); // Variable name and function parameter
      });

      // The div outside the code block should be wrapped
      await waitFor(() => {
        expect(screen.getByText(/This should be wrapped/)).toBeInTheDocument();
      });
    });

    it('handles mixed safe and unsafe content', async () => {
      const content = `# Mixed Content Test

1. Auto-link: <https://block.dev>
2. Inline code: \`const x = Array<T>();\`
3. Real markup: <input type="text" disabled>
4. Placeholder path: <project-root>/src`;

      renderWithIntl(<MarkdownContent content={content} />);

      await waitFor(() => {
        expect(screen.getByText('Mixed Content Test')).toBeInTheDocument();
        expect(screen.getByText(/Auto-link/)).toBeInTheDocument();
        expect(screen.getByText(/Inline code/)).toBeInTheDocument();
        expect(screen.getByText(/Real markup/)).toBeInTheDocument();
        expect(screen.getByText(/Placeholder path/)).toBeInTheDocument();
      });

      // Only the input tag should be wrapped
      await waitFor(() => {
        expect(screen.getByText(/input/)).toBeInTheDocument();
        expect(screen.getByText(/type/)).toBeInTheDocument();
        expect(screen.getByText(/disabled/)).toBeInTheDocument();
      });

      // Should not have actual input elements in the DOM
      const inputElements = document.querySelectorAll('input');
      expect(inputElements).toHaveLength(0);
    });
  });

  describe('Code Block Functionality', () => {
    it('renders code blocks with syntax highlighting', async () => {
      const content = `\`\`\`javascript
console.log('Hello, World!');
\`\`\``;

      renderWithIntl(<MarkdownContent content={content} />);

      await waitFor(() => {
        expect(screen.getByText(/console/)).toBeInTheDocument();
        expect(screen.getByText(/log/)).toBeInTheDocument();
        expect(screen.getByText(/Hello, World!/)).toBeInTheDocument();
      });
    });

    it('renders inline code', async () => {
      const content = 'Use `console.log()` to debug.';

      renderWithIntl(<MarkdownContent content={content} />);

      await waitFor(() => {
        expect(screen.getByText(/Use/)).toBeInTheDocument();
        expect(screen.getByText(/to debug/)).toBeInTheDocument();
        expect(screen.getByText('console.log()')).toBeInTheDocument();
      });
    });

    it('renders untagged fenced code blocks (no language) through the same CodeBlock component as tagged blocks', async () => {
      const plainTextBlock = [
        'first line of plain text',
        'second line of plain text',
        '',
        'line after a blank line',
      ].join('\n');
      const content = ['```', plainTextBlock, '```'].join('\n');

      const { container } = renderWithIntl(<MarkdownContent content={content} />);

      await waitFor(() => {
        expect(screen.getByText(/first line of plain text/)).toBeInTheDocument();
      });

      // There should be exactly one <pre> element wrapping exactly one <code>
      // element that contains the entire multi-line block as a single node,
      // rather than one <code>/<pre> pairing per line.
      const preElements = container.querySelectorAll('pre');
      expect(preElements).toHaveLength(1);

      const codeElementsInsidePre = preElements[0].querySelectorAll('code');
      expect(codeElementsInsidePre).toHaveLength(1);
      expect(codeElementsInsidePre[0].textContent).toContain('first line of plain text');
      expect(codeElementsInsidePre[0].textContent).toContain('line after a blank line');

      // The block should NOT use the small single-line "inline code" badge
      // style - that would indicate the bug regressed.
      expect(codeElementsInsidePre[0].className).not.toContain('bg-inline-code');

      // It should get the same hover "copy" button that tagged code blocks
      // get, since it now renders through the shared CodeBlock component.
      expect(preElements[0].querySelector('[data-testid="copy-icon"]')).toBeInTheDocument();
    });
  });

  describe('Markdown Features', () => {
    it('renders headers correctly', async () => {
      const content = `# H1 Header
## H2 Header
### H3 Header`;

      renderWithIntl(<MarkdownContent content={content} />);

      await waitFor(() => {
        expect(screen.getByRole('heading', { level: 1, name: 'H1 Header' })).toBeInTheDocument();
        expect(screen.getByRole('heading', { level: 2, name: 'H2 Header' })).toBeInTheDocument();
        expect(screen.getByRole('heading', { level: 3, name: 'H3 Header' })).toBeInTheDocument();
      });
    });

    it('renders lists correctly', async () => {
      const content = `- Item 1
- Item 2
- Item 3

1. Numbered 1
2. Numbered 2`;

      renderWithIntl(<MarkdownContent content={content} />);

      await waitFor(() => {
        expect(screen.getByText('Item 1')).toBeInTheDocument();
        expect(screen.getByText('Item 2')).toBeInTheDocument();
        expect(screen.getByText('Item 3')).toBeInTheDocument();
        expect(screen.getByText('Numbered 1')).toBeInTheDocument();
        expect(screen.getByText('Numbered 2')).toBeInTheDocument();
      });
    });

    it('renders links with correct attributes', async () => {
      const content = '[Visit Block](https://block.dev)';

      renderWithIntl(<MarkdownContent content={content} />);

      await waitFor(() => {
        const link = screen.getByRole('link', { name: 'Visit Block' });
        expect(link).toBeInTheDocument();
        expect(link).toHaveAttribute('href', 'https://block.dev');
        expect(link).toHaveAttribute('target', '_blank');
        expect(link).toHaveAttribute('rel', 'noopener noreferrer');
      });
    });

    it('delegates unknown protocol confirmation to the main process once', async () => {
      const openExternal = vi.fn().mockResolvedValue('cancelled');
      window.electron.openExternal = openExternal;
      renderWithIntl(<MarkdownContent content="[Open app](custom-handler:resource)" />);

      fireEvent.click(await screen.findByRole('link', { name: 'Open app' }));

      await waitFor(() => {
        expect(openExternal).toHaveBeenCalledOnce();
        expect(openExternal).toHaveBeenCalledWith('custom-handler:resource');
      });
      expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    });

    it('renders tables correctly', async () => {
      const content = `| Name | Value |
|------|-------|
| Test | 123   |
| Demo | 456   |`;

      renderWithIntl(<MarkdownContent content={content} />);

      await waitFor(() => {
        expect(screen.getByText('Name')).toBeInTheDocument();
        expect(screen.getByText('Value')).toBeInTheDocument();
        expect(screen.getByText('Test')).toBeInTheDocument();
        expect(screen.getByText('123')).toBeInTheDocument();
        expect(screen.getByText('Demo')).toBeInTheDocument();
        expect(screen.getByText('456')).toBeInTheDocument();
      });
    });
  });

  describe('Error Handling', () => {
    it('handles empty content gracefully', async () => {
      renderWithIntl(<MarkdownContent content="" />);

      // Should not throw and should render the component
      const container = document.querySelector('.w-full.overflow-x-hidden');
      expect(container).toBeInTheDocument();
    });

    it('handles malformed markdown gracefully', async () => {
      const content = `# Unclosed header
[Unclosed link(https://example.com
\`\`\`
Unclosed code block`;

      renderWithIntl(<MarkdownContent content={content} />);

      await waitFor(() => {
        // Should still render what it can
        expect(screen.getByText('Unclosed header')).toBeInTheDocument();
      });
    });
  });

  describe('Line Break Functionality', () => {
    it('preserves single line breaks with remark-breaks plugin', async () => {
      const content = `First line
Second line
Third line`;

      const { container } = renderWithIntl(<MarkdownContent content={content} />);

      await waitFor(() => {
        // Check that all text content is present (text may be split by <br> tags)
        expect(container).toHaveTextContent('First line');
        expect(container).toHaveTextContent('Second line');
        expect(container).toHaveTextContent('Third line');
      });

      // Check that line breaks are preserved (rendered as <br> tags)
      const brElements = container.querySelectorAll('br');
      expect(brElements.length).toBeGreaterThan(0);
    });

    it('handles mixed content with line breaks', async () => {
      const content = `# Header
Paragraph with
line breaks.

- List item 1
- List item 2`;

      const { container } = renderWithIntl(<MarkdownContent content={content} />);

      await waitFor(() => {
        expect(screen.getByRole('heading', { level: 1, name: 'Header' })).toBeInTheDocument();

        // Check that text content is present (text may be split by <br> tags)
        expect(container).toHaveTextContent('Paragraph with');
        expect(container).toHaveTextContent('line breaks.');
        expect(screen.getByText('List item 1')).toBeInTheDocument();
        expect(screen.getByText('List item 2')).toBeInTheDocument();
      });
    });

    it('maintains existing markdown features with line breaks', async () => {
      const content = `**Bold text**
with line break

\`code\` and
more text`;

      const { container } = renderWithIntl(<MarkdownContent content={content} />);

      await waitFor(() => {
        // Bold text should still work
        const boldElement = container.querySelector('strong');
        expect(boldElement).toBeInTheDocument();
        expect(boldElement).toHaveTextContent('Bold text');

        // Code should still work
        expect(screen.getByText('code')).toBeInTheDocument();
      });
    });
  });

  describe('URL Overflow Handling', () => {
    it('handles very long URLs without overflow', async () => {
      const longUrl =
        'https://example-docs.com/document/d/1oruk3lcrnhoOXMFzBJB8X6qQ5AtQTmj4XXxXk3xK-3g/edit?usp=sharing&mode=edit&version=1';
      const content = `Check out this document: ${longUrl}

Another very long URL: https://www.example.com/very/long/path/with/many/segments/and/parameters?param1=value1&param2=value2&param3=value3&param4=value4&param5=value5`;

      const { container } = renderWithIntl(<MarkdownContent content={content} />);

      await waitFor(() => {
        expect(screen.getByText(/Check out this document/)).toBeInTheDocument();
        expect(screen.getByText(/Another very long URL/)).toBeInTheDocument();
      });

      // Check that URLs are rendered as links
      const links = container.querySelectorAll('a');
      expect(links.length).toBeGreaterThan(0);

      // Check that links have proper CSS classes for word breaking
      links.forEach((link) => {
        // The CSS should allow the text to break
        expect(link).toBeInTheDocument();
      });
    });

    it('handles markdown links with long URLs', async () => {
      const longUrl =
        'https://example-docs.com/document/d/1oruk3lcrnhoOXMFzBJB8X6qQ5AtQTmj4XXxXk3xK-3g/edit?usp=sharing&mode=edit&version=1';
      const content = `[Click here for the document](${longUrl})`;

      renderWithIntl(<MarkdownContent content={content} />);

      await waitFor(() => {
        const link = screen.getByRole('link', { name: 'Click here for the document' });
        expect(link).toBeInTheDocument();
        expect(link).toHaveAttribute('href', longUrl);
      });
    });

    it('handles multiple long URLs in the same message', async () => {
      const content = `Here are some long URLs:

1. Example Doc: https://example-docs.com/document/d/1oruk3lcrnhoOXMFzBJB8X6qQ5AtQTmj4XXxXk3xK-3g/edit?usp=sharing&mode=edit&version=1
2. Another long URL: https://www.example.com/very/long/path/with/many/segments/and/parameters?param1=value1&param2=value2&param3=value3
3. Third URL: https://api.example.com/v1/users/12345/documents/67890/attachments/abcdef123456789?format=json&include=metadata&sort=created_at`;

      renderWithIntl(<MarkdownContent content={content} />);

      await waitFor(() => {
        expect(screen.getByText(/Here are some long URLs/)).toBeInTheDocument();
        expect(screen.getByText(/Example Doc/)).toBeInTheDocument();
        expect(screen.getByText(/Another long URL/)).toBeInTheDocument();
        expect(screen.getByText(/Third URL/)).toBeInTheDocument();
      });
    });

    it('applies word-break CSS classes to the container', () => {
      const content = 'Test content';
      renderWithIntl(<MarkdownContent content={content} />);

      const markdownContainer = document.querySelector('.prose');
      expect(markdownContainer).toBeInTheDocument();
      expect(markdownContainer).toHaveClass('prose-a:break-all');
      expect(markdownContainer).toHaveClass('prose-a:overflow-wrap-anywhere');
    });
  });

  describe('KaTeX Math Rendering - singleDollarTextMath: false', () => {
    it('treats single dollar signs as plain text', async () => {
      const content = 'The formula $x_i$ represents the i-th element.';

      const { container } = renderWithIntl(<MarkdownContent content={content} />);

      await waitFor(() => {
        const katexElements = container.querySelectorAll('.katex');
        expect(katexElements.length).toBe(0);
        expect(container).toHaveTextContent('$x_i$');
      });
    });

    it('renders double dollar signs as display math', async () => {
      const content = `Calculate

$$
x^2 + y^2
$$

for the result.`;

      const { container } = renderWithIntl(<MarkdownContent content={content} />);

      await waitFor(() => {
        const katexDisplay = container.querySelector('.katex-display');
        expect(katexDisplay).toBeInTheDocument();
      });
    });

    it('handles shell commands without triggering math mode', async () => {
      const content = 'Run echo "$FOO_BAR" to see the value.';

      const { container } = renderWithIntl(<MarkdownContent content={content} />);

      await waitFor(() => {
        const katexElements = container.querySelectorAll('.katex');
        expect(katexElements.length).toBe(0);
        expect(container).toHaveTextContent('$FOO_BAR');
      });
    });

    it('preserves math in code blocks', async () => {
      const content = 'The formula `math\nx^2\n` uses inline code.';

      const { container } = renderWithIntl(<MarkdownContent content={content} />);

      await waitFor(() => {
        expect(container).toHaveTextContent('x^2');
      });
    });
  });
});
