import { beforeEach, describe, expect, it, vi } from 'vitest';
import { acpSaveProviderConfig } from '../../../../../../acp/providers';
import { providerConfigSubmitHandler } from './DefaultSubmitHandler';

vi.mock('../../../../../../acp/providers', () => ({
  acpSaveProviderConfig: vi.fn(),
}));

const litellm = {
  name: 'litellm',
  metadata: {
    config_keys: [
      { name: 'LITELLM_API_KEY' },
      { name: 'LITELLM_HOST', default: 'http://localhost:4000' },
      { name: 'LITELLM_BASE_PATH', default: 'v1/chat/completions' },
      { name: 'LITELLM_TIMEOUT', default: '600' },
    ],
  },
};

function savedKeys() {
  const [, fields] = vi.mocked(acpSaveProviderConfig).mock.calls[0];
  return Object.fromEntries(fields.map(({ key, value }) => [key, value]));
}

describe('providerConfigSubmitHandler', () => {
  beforeEach(() => {
    vi.mocked(acpSaveProviderConfig).mockClear();
  });

  it('never writes a metadata default for a field the user did not supply', async () => {
    await providerConfigSubmitHandler(litellm, { LITELLM_API_KEY: 'rotated-key' });

    expect(savedKeys()).toEqual({ LITELLM_API_KEY: 'rotated-key' });
  });

  it('submits only the values it was given', async () => {
    await providerConfigSubmitHandler(litellm, {
      LITELLM_API_KEY: 'rotated-key',
      LITELLM_HOST: 'http://192.168.1.50:4000',
    });

    expect(savedKeys()).toEqual({
      LITELLM_API_KEY: 'rotated-key',
      LITELLM_HOST: 'http://192.168.1.50:4000',
    });
  });

  it('skips empty values rather than falling back to the default', async () => {
    await providerConfigSubmitHandler(litellm, {
      LITELLM_API_KEY: 'rotated-key',
      LITELLM_HOST: '',
    });

    expect(savedKeys()).toEqual({ LITELLM_API_KEY: 'rotated-key' });
  });

  it('ignores values for keys the provider does not declare', async () => {
    await providerConfigSubmitHandler(litellm, {
      LITELLM_API_KEY: 'rotated-key',
      SOME_OTHER_KEY: 'ignored',
    });

    expect(savedKeys()).toEqual({ LITELLM_API_KEY: 'rotated-key' });
  });
});
