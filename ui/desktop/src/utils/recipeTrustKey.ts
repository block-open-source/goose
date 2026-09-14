import crypto from 'crypto';

// Trust is scoped to the effective recipe: the template plus the parameter values
// that will be substituted into it. Two deeplinks that share a template but supply
// different parameters (e.g. a different `{{script}}`) must not share trust, or a
// once-approved template would silently execute new attacker-supplied values.
//
// When no parameters are supplied the key is sha256(JSON.stringify(recipe)) exactly
// as before, so existing acceptances for non-parameterized recipes stay valid.
export function recipeTrustKey(recipe: unknown, parameters?: Record<string, string>): string {
  const hash = crypto.createHash('sha256');
  if (parameters && Object.keys(parameters).length > 0) {
    const sorted = Object.fromEntries(
      Object.entries(parameters).sort(([a], [b]) => a.localeCompare(b))
    );
    hash.update(JSON.stringify({ recipe, parameters: sorted }));
  } else {
    hash.update(JSON.stringify(recipe));
  }
  return hash.digest('hex');
}
