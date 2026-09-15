/**
 * @vitest-environment jsdom
 */
import { act, render } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import Hub from './Hub';
import { IntlTestWrapper } from '../i18n/test-utils';
import type { UserInput } from '../types/message';

const captured = vi.hoisted(() => ({
  handleSubmit: null as ((input: UserInput) => void) | null,
  onWorkingDirChange: null as ((dir: string) => void) | null,
}));
const mockSetView = vi.fn();

vi.mock('./ChatInput', () => ({
  default: (props: {
    handleSubmit: (input: UserInput) => void;
    onWorkingDirChange?: (dir: string) => void;
  }) => {
    captured.handleSubmit = props.handleSubmit;
    captured.onWorkingDirChange = props.onWorkingDirChange ?? null;
    return <div />;
  },
}));
vi.mock('./ConfigContext', () => ({ useConfig: () => ({ extensionsList: [] }) }));
vi.mock('../utils/workingDir', () => ({
  getInitialWorkingDir: () => '/tmp/goose',
  getEffectiveWorkingDir: () => Promise.resolve('/tmp/effective'),
}));
vi.mock('../utils/nextChatExtensions', () => ({
  createNextChatExtensionDraft: () => ({}),
  selectNextChatExtensions: () => [],
}));

beforeEach(() => {
  vi.clearAllMocks();
  captured.handleSubmit = null;
  captured.onWorkingDirChange = null;
});

describe('Hub', () => {
  it('navigates immediately and leaves the effective working directory for Pair to resolve', () => {
    const draftRef = { current: { msg: 'hello from hub', images: [] } };
    render(
      <IntlTestWrapper>
        <Hub setView={mockSetView} draftRef={draftRef} />
      </IntlTestWrapper>
    );

    act(() => captured.handleSubmit?.({ msg: draftRef.current.msg, images: [] }));

    expect(mockSetView).toHaveBeenCalledWith('pair', {
      disableAnimation: true,
      initialMessage: { msg: 'hello from hub', images: [] },
      workingDir: undefined,
      allExtensions: [],
    });
    expect(draftRef.current).toEqual({ msg: 'hello from hub', images: [] });
  });

  it('passes a user-selected working directory through to pair', () => {
    const draftRef = { current: { msg: 'hello from hub', images: [] } };
    render(
      <IntlTestWrapper>
        <Hub setView={mockSetView} draftRef={draftRef} />
      </IntlTestWrapper>
    );

    act(() => captured.onWorkingDirChange?.('/tmp/picked'));
    act(() => captured.handleSubmit?.({ msg: draftRef.current.msg, images: [] }));

    expect(mockSetView).toHaveBeenCalledWith('pair', {
      disableAnimation: true,
      initialMessage: { msg: 'hello from hub', images: [] },
      workingDir: '/tmp/picked',
      allExtensions: [],
    });
  });
});
