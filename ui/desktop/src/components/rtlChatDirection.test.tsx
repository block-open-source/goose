import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import MarkdownContent from './MarkdownContent';
import MessageQueue, { type QueuedMessage } from './MessageQueue';
import UserMessage from './UserMessage';
import { IntlTestWrapper } from '../i18n/test-utils';
import type { Message } from '../types/message';

vi.mock('./icons', () => ({
  Check: () => <div data-testid="check-icon">ok</div>,
  Copy: () => <div data-testid="copy-icon">copy</div>,
  Edit: () => <div data-testid="edit-icon">edit</div>,
  Close: () => <div data-testid="close-icon">close</div>,
}));

const arabicText = 'هذه رسالة تجريبية باللغة العربية';
const hebrewText = 'זו הודעת בדיקה בעברית';

function renderWithIntl(ui: React.ReactElement) {
  return render(ui, { wrapper: IntlTestWrapper });
}

function makeUserMessage(text: string): Message {
  return {
    content: [{ type: 'text', text }],
    created: Date.now(),
    id: 'msg-1',
    metadata: { agentVisible: true, userVisible: true },
    role: 'user',
  };
}

describe('RTL chat direction', () => {
  describe('MarkdownContent per-block direction', () => {
    it('tags each block with its own direction in a mixed message', async () => {
      const { container } = renderWithIntl(
        <MarkdownContent content={`${arabicText}\n\nThis is an English paragraph.`} />
      );

      await waitFor(() => {
        const paragraphs = container.querySelectorAll('p');
        expect(paragraphs).toHaveLength(2);
      });

      const paragraphs = container.querySelectorAll('p');
      expect(paragraphs[0].getAttribute('dir')).toBe('rtl');
      expect(paragraphs[1].getAttribute('dir')).toBe('ltr');
    });

    it('keeps paragraphs without strong characters undirected', async () => {
      const { container } = renderWithIntl(<MarkdownContent content="12345" />);

      await waitFor(() => {
        expect(container.querySelector('p')).toBeInTheDocument();
      });
      expect(container.querySelector('p')?.getAttribute('dir')).toBe(null);
    });

    it('does not let a long inline identifier flip an RTL paragraph', async () => {
      const { container } = renderWithIntl(
        <MarkdownContent content="شغّل `getPredefinedModelsFromEnv` في المجلد" />
      );

      await waitFor(() => {
        expect(container.querySelector('p')).toBeInTheDocument();
      });
      expect(container.querySelector('p')?.getAttribute('dir')).toBe('rtl');
    });

    it('does not let a fenced code block flip a list item direction', async () => {
      const content = `- هذه قائمة بالعربية

  \`\`\`js
  const veryLongEnglishCode = 1;
  \`\`\`
`;
      const { container } = renderWithIntl(<MarkdownContent content={content} />);

      await waitFor(() => {
        expect(container.querySelector('li')).toBeInTheDocument();
      });
      // Loose list: the item's prose lives in a <p>, and the li's own vote
      // counts it, so the marker side follows the prose direction.
      expect(container.querySelector('li')?.getAttribute('dir')).toBe('rtl');
      expect(container.querySelector('li p')?.getAttribute('dir')).toBe('rtl');
    });

    it('tags loose list items even when the message-level vote is LTR', async () => {
      const content =
        'A fairly long English intro paragraph that tips the raw message text towards LTR.\n\n- عنصر عربي\n\n- عنصر آخر';
      const { container } = renderWithIntl(<MarkdownContent content={content} />);

      await waitFor(() => {
        expect(container.querySelector('li')).toBeInTheDocument();
      });
      expect(container.querySelector('li')?.getAttribute('dir')).toBe('rtl');
    });

    it('does not let a nested sublist flip a list item direction', async () => {
      const content =
        '- هذه قائمة عربية قصيرة\n  - a fairly long english nested sublist entry here';
      const { container } = renderWithIntl(<MarkdownContent content={content} />);

      await waitFor(() => {
        expect(container.querySelector('li')).toBeInTheDocument();
      });
      expect(container.querySelector('li')?.getAttribute('dir')).toBe('rtl');
    });

    it('does not let a long KaTeX formula flip an RTL paragraph', async () => {
      const content = 'القيمة المحسوبة $$\\text{calculateTotalSumOfAllItems}$$ في القائمة';
      const { container } = renderWithIntl(<MarkdownContent content={content} />);

      await waitFor(() => {
        expect(container.querySelector('p')).toBeInTheDocument();
      });
      expect(container.querySelector('p')?.getAttribute('dir')).toBe('rtl');
    });

    it('renders inline code LTR inside an RTL paragraph', async () => {
      const { container } = renderWithIntl(
        <MarkdownContent content={`${arabicText} مع \`some code\``} />
      );

      await waitFor(() => {
        expect(container.querySelector('code')).toBeInTheDocument();
      });
      expect(container.querySelector('code')?.getAttribute('dir')).toBe('ltr');
      expect(container.querySelector('p')?.getAttribute('dir')).toBe('rtl');
    });

    it('renders fenced code blocks LTR', async () => {
      const { container } = renderWithIntl(
        <MarkdownContent content={`${arabicText}\n\n\`\`\`js\nconst x = 1;\n\`\`\``} />
      );

      await waitFor(() => {
        expect(container.querySelector('div[dir="ltr"]')).toBeInTheDocument();
      });
      expect(container.querySelector('p')?.getAttribute('dir')).toBe('rtl');
    });
  });

  describe('UserMessage', () => {
    it('sets dir on the message bubble', () => {
      const { container } = renderWithIntl(<UserMessage message={makeUserMessage(arabicText)} />);
      expect(container.querySelector('.user-message-bubble')?.getAttribute('dir')).toBe('rtl');
    });

    it('leaves the dir attribute off non-directional messages', () => {
      const { container } = renderWithIntl(<UserMessage message={makeUserMessage('123')} />);
      expect(container.querySelector('.user-message-bubble')?.getAttribute('dir')).toBe(null);
    });

    it('updates the edit textarea direction as the content changes', () => {
      renderWithIntl(<UserMessage message={makeUserMessage('hello')} />);

      fireEvent.click(screen.getByRole('button', { name: /Edit message/ }));
      const textarea = screen.getByRole('textbox') as HTMLTextAreaElement;

      expect(textarea.getAttribute('dir')).toBe('ltr');
      fireEvent.change(textarea, { target: { value: hebrewText } });
      expect(textarea.getAttribute('dir')).toBe('rtl');
    });
  });

  describe('MessageQueue', () => {
    function renderQueueWith(content: string) {
      const queued: QueuedMessage[] = [{ id: 'q1', content, timestamp: Date.now(), images: [] }];
      renderWithIntl(
        <MessageQueue queuedMessages={queued} onRemoveMessage={() => {}} onClearQueue={() => {}} />
      );
    }

    it('sets dir on the queued message preview', () => {
      renderQueueWith(arabicText);
      expect(screen.getByText(arabicText).getAttribute('dir')).toBe('rtl');
    });

    it('applies direction to the collapsed queue preview', () => {
      renderQueueWith(arabicText);
      fireEvent.click(screen.getByTitle('Collapse queue'));

      expect(screen.getByTitle(arabicText).getAttribute('dir')).toBe('rtl');
    });

    it('follows the direction while editing a queued message', () => {
      renderQueueWith(arabicText);
      fireEvent.click(screen.getByText(arabicText));

      const textarea = screen.getByRole('textbox') as HTMLTextAreaElement;
      expect(textarea.getAttribute('dir')).toBe('rtl');

      fireEvent.change(textarea, { target: { value: 'hello again' } });
      expect(textarea.getAttribute('dir')).toBe('ltr');
    });
  });
});
