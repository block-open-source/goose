import { beforeEach, describe, expect, it, vi } from 'vitest';
import { getAcpClient } from '../acpConnection';
import { getSessionExtensions, removeSessionExtension } from '../session-extensions';

vi.mock('../acpConnection', () => ({
  getAcpClient: vi.fn(),
}));

const extension = (name: string, extensionKey: string) => ({
  extension: {
    type: 'builtin' as const,
    name,
  },
  extensionKey,
});

describe('ACP session extensions', () => {
  const list = vi.fn();
  const remove = vi.fn();

  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(getAcpClient).mockResolvedValue({
      goose: {
        sessionExtensionsList_unstable: list,
        sessionExtensionsRemove_unstable: remove,
      },
    } as unknown as Awaited<ReturnType<typeof getAcpClient>>);
  });

  it('preserves the backend identity on mapped session entries', async () => {
    list.mockResolvedValue({ extensions: [extension('i\u0307', 'i_')] });

    await expect(getSessionExtensions('session')).resolves.toEqual([
      expect.objectContaining({ name: 'i\u0307', extensionKey: 'i_' }),
    ]);
  });

  it('rejects duplicate authoritative identities', async () => {
    list.mockResolvedValue({
      extensions: [extension('first', 'duplicate'), extension('second', 'duplicate')],
    });

    await expect(getSessionExtensions('session')).rejects.toThrow(
      "Duplicate session extension key 'duplicate'"
    );
  });

  it('removes by the backend identity', async () => {
    await removeSessionExtension('session', 'i_');

    expect(remove).toHaveBeenCalledWith({ sessionId: 'session', extensionKey: 'i_' });
  });
});
