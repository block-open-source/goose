import type { RequestRecipeParams_unstable } from '@aaif/goose-acp-client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

type RecipeParamRequestsModule = typeof import('../recipeParamRequests');

function recipeParamRequest(
  sessionId = 'session-1',
  parameterScopeId?: string
): RequestRecipeParams_unstable {
  return {
    sessionId,
    parameterScopeId,
    parameters: [
      {
        key: 'topic',
        description: 'Topic',
        input_type: 'string',
        requirement: 'user_prompt',
      },
    ],
  };
}

function setRecipeParameters(values: Record<string, string>) {
  const get = vi.fn((key: string) => (key === 'recipeParameters' ? values : undefined));
  Object.defineProperty(window, 'appConfig', {
    configurable: true,
    value: { get },
  });
  return get;
}

describe('ACP recipe param requests', () => {
  let requests: RecipeParamRequestsModule;

  beforeEach(async () => {
    vi.resetModules();
    requests = await import('../recipeParamRequests');
  });

  afterEach(() => {
    for (const request of requests.getAcpRecipeParamRequestsSnapshot()) {
      requests.cancelAcpRecipeParamRequest(request.id);
    }
    Reflect.deleteProperty(window, 'appConfig');
  });

  it('keeps ordinary requests pending without startup values', async () => {
    setRecipeParameters({ topic: 'release notes' });

    const response = requests.requestAcpRecipeParams(recipeParamRequest());
    const [pendingRequest] = requests.getAcpRecipeParamRequestsSnapshot();

    expect(pendingRequest.initialValues).toEqual({});
    requests.resolveAcpRecipeParamRequest(pendingRequest.id, { topic: 'manual value' });
    await expect(response).resolves.toEqual({
      action: 'submit',
      values: { topic: 'manual value' },
    });
  });

  it('offers startup values only to the callback carrying the deeplink scope id', async () => {
    setRecipeParameters({ topic: 'release notes' });
    const scope = requests.beginConfiguredRecipeParameterScope()!;

    const otherResponse = requests.requestAcpRecipeParams(recipeParamRequest('session-2'));
    const ownerResponse = requests.requestAcpRecipeParams(
      recipeParamRequest('session-1', scope.id)
    );
    const pending = requests.getAcpRecipeParamRequestsSnapshot();

    expect(pending.find((request) => request.sessionId === 'session-2')?.initialValues).toEqual({});
    expect(pending.find((request) => request.sessionId === 'session-1')?.initialValues).toEqual({
      topic: 'release notes',
    });

    for (const request of pending) {
      requests.cancelAcpRecipeParamRequest(request.id);
    }
    await expect(otherResponse).resolves.toEqual({ action: 'cancel' });
    await expect(ownerResponse).resolves.toEqual({ action: 'cancel' });
    scope.finish();
  });

  it('does not offer deeplink values for file parameters', async () => {
    setRecipeParameters({ document: '/tmp/private.txt', topic: 'release notes' });
    const scope = requests.beginConfiguredRecipeParameterScope()!;
    const request = recipeParamRequest('session-1', scope.id);
    request.parameters.unshift({
      key: 'document',
      description: 'Document',
      input_type: 'file',
      requirement: 'required',
    });

    const response = requests.requestAcpRecipeParams(request);
    const [pendingRequest] = requests.getAcpRecipeParamRequestsSnapshot();

    expect(pendingRequest.initialValues).toEqual({ topic: 'release notes' });

    requests.cancelAcpRecipeParamRequest(pendingRequest.id);
    await expect(response).resolves.toEqual({ action: 'cancel' });
    scope.finish();
  });

  it('allows same-session retries until a terminal response consumes the values', async () => {
    setRecipeParameters({ topic: 'release notes' });
    const scope = requests.beginConfiguredRecipeParameterScope()!;

    const firstResponse = requests.requestAcpRecipeParams(
      recipeParamRequest('session-1', scope.id)
    );
    const retryResponse = requests.requestAcpRecipeParams(
      recipeParamRequest('session-1', scope.id)
    );
    const [firstRequest, retryRequest] = requests.getAcpRecipeParamRequestsSnapshot();

    expect(firstRequest.initialValues).toEqual({ topic: 'release notes' });
    expect(retryRequest.initialValues).toEqual({ topic: 'release notes' });

    requests.resolveAcpRecipeParamRequest(firstRequest.id, { topic: 'release notes' });
    await expect(firstResponse).resolves.toEqual({
      action: 'submit',
      values: { topic: 'release notes' },
    });
    expect(requests.getAcpRecipeParamRequestsSnapshot()[0].initialValues).toEqual({});

    requests.cancelAcpRecipeParamRequest(retryRequest.id);
    await expect(retryResponse).resolves.toEqual({ action: 'cancel' });
    scope.finish();
  });

  it('does not reuse startup values after submission', async () => {
    setRecipeParameters({ topic: 'release notes' });
    const scope = requests.beginConfiguredRecipeParameterScope()!;

    const firstResponse = requests.requestAcpRecipeParams(
      recipeParamRequest('session-1', scope.id)
    );
    const [firstRequest] = requests.getAcpRecipeParamRequestsSnapshot();
    requests.resolveAcpRecipeParamRequest(firstRequest.id, { topic: 'release notes' });
    await firstResponse;

    const laterResponse = requests.requestAcpRecipeParams(
      recipeParamRequest('session-1', scope.id)
    );
    const [laterRequest] = requests.getAcpRecipeParamRequestsSnapshot();
    expect(laterRequest.initialValues).toEqual({});

    requests.cancelAcpRecipeParamRequest(laterRequest.id);
    await laterResponse;
    scope.finish();
  });

  it('does not reuse startup values after cancellation', async () => {
    setRecipeParameters({ topic: 'release notes' });
    const scope = requests.beginConfiguredRecipeParameterScope()!;

    const firstResponse = requests.requestAcpRecipeParams(
      recipeParamRequest('session-1', scope.id)
    );
    const [firstRequest] = requests.getAcpRecipeParamRequestsSnapshot();
    requests.cancelAcpRecipeParamRequest(firstRequest.id);
    await firstResponse;

    const laterResponse = requests.requestAcpRecipeParams(
      recipeParamRequest('session-1', scope.id)
    );
    const [laterRequest] = requests.getAcpRecipeParamRequestsSnapshot();
    expect(laterRequest.initialValues).toEqual({});

    requests.cancelAcpRecipeParamRequest(laterRequest.id);
    await laterResponse;
    scope.finish();
  });

  it('does not let an unrelated cancellation consume the owner values', async () => {
    setRecipeParameters({ topic: 'release notes' });
    const scope = requests.beginConfiguredRecipeParameterScope()!;

    const ownerResponse = requests.requestAcpRecipeParams(
      recipeParamRequest('session-1', scope.id)
    );
    const otherResponse = requests.requestAcpRecipeParams(recipeParamRequest('session-2'));
    const otherRequest = requests
      .getAcpRecipeParamRequestsSnapshot()
      .find((request) => request.sessionId === 'session-2')!;
    requests.cancelAcpRecipeParamRequest(otherRequest.id);
    await otherResponse;

    const retryResponse = requests.requestAcpRecipeParams(
      recipeParamRequest('session-1', scope.id)
    );
    const ownerRequests = requests
      .getAcpRecipeParamRequestsSnapshot()
      .filter((request) => request.sessionId === 'session-1');
    expect(ownerRequests).toHaveLength(2);
    expect(ownerRequests[1].initialValues).toEqual({ topic: 'release notes' });

    for (const request of ownerRequests) {
      requests.cancelAcpRecipeParamRequest(request.id);
    }
    await ownerResponse;
    await retryResponse;
    scope.finish();
  });

  it('consumes startup values when a scope finishes without a callback', async () => {
    setRecipeParameters({ topic: 'release notes' });
    const scope = requests.beginConfiguredRecipeParameterScope()!;
    scope.finish();

    const laterResponse = requests.requestAcpRecipeParams(
      recipeParamRequest('session-1', scope.id)
    );
    const [laterRequest] = requests.getAcpRecipeParamRequestsSnapshot();
    expect(laterRequest.initialValues).toEqual({});

    requests.cancelAcpRecipeParamRequest(laterRequest.id);
    await laterResponse;
  });

  it('reads app configuration only once after consumption', async () => {
    const get = setRecipeParameters({ topic: 'release notes' });
    const scope = requests.beginConfiguredRecipeParameterScope()!;
    scope.finish();

    const secondScope = requests.beginConfiguredRecipeParameterScope();
    const laterResponse = requests.requestAcpRecipeParams(recipeParamRequest());
    const [laterRequest] = requests.getAcpRecipeParamRequestsSnapshot();

    expect(get).toHaveBeenCalledTimes(1);
    expect(secondScope).toBeUndefined();
    expect(laterRequest.initialValues).toEqual({});

    requests.cancelAcpRecipeParamRequest(laterRequest.id);
    await laterResponse;
  });
});
