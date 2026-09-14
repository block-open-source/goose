import { describe, expect, it } from 'vitest';
import crypto from 'crypto';
import { recipeTrustKey } from './recipeTrustKey';

const recipe = {
  version: '1.0.0',
  title: 'Parameterized',
  description: 'runs {{script}}',
  extensions: [{ type: 'stdio', name: 'x', cmd: 'sh', args: ['-c', '{{script}}'] }],
};

describe('recipeTrustKey', () => {
  it('is backward compatible with sha256(JSON.stringify(recipe)) when no parameters', () => {
    const legacy = crypto.createHash('sha256').update(JSON.stringify(recipe)).digest('hex');
    expect(recipeTrustKey(recipe)).toBe(legacy);
    expect(recipeTrustKey(recipe, {})).toBe(legacy);
  });

  it('changes when a parameter value changes on the same template', () => {
    const safe = recipeTrustKey(recipe, { script: 'echo safe' });
    const evil = recipeTrustKey(recipe, { script: 'rm -rf ~' });
    expect(safe).not.toBe(evil);
  });

  it('is stable for the same template and parameters regardless of key order', () => {
    const a = recipeTrustKey(recipe, { script: 'ls', dir: '/tmp' });
    const b = recipeTrustKey(recipe, { dir: '/tmp', script: 'ls' });
    expect(a).toBe(b);
  });

  it('differs from the no-parameter key even when parameters are present', () => {
    expect(recipeTrustKey(recipe, { script: 'ls' })).not.toBe(recipeTrustKey(recipe));
  });
});
