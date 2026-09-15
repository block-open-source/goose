const assert = require('node:assert/strict');
const test = require('node:test');

const {renderDocumentation, schemaType} = require('./generate-acp-docs');

const schema = {
  $defs: {
    Request: {
      type: 'object',
      description: 'Run the example.',
      properties: {
        target: {$ref: '#/$defs/Target'},
        tags: {type: ['array', 'null'], items: {type: 'string'}},
        values: {type: 'object', additionalProperties: {type: 'integer'}},
        mode: {enum: ['fast', 'safe'], default: 'safe'},
        count: {type: 'integer', minimum: 1, maximum: 10},
        label: {type: 'string', format: 'slug', pattern: '^[a-z]+$'},
      },
      required: ['target'],
    },
    Response: {type: 'object'},
    Target: {
      oneOf: [
        {$ref: '#/$defs/Response', type: 'object', properties: {kind: {const: 'object'}}},
        {type: 'string'},
      ],
    },
  },
};
const meta = {
  methods: [{method: '_goose/example', requestType: 'Request', responseType: 'Response'}],
  agentRequests: [],
  notifications: [],
};

test('renders representative schema forms deterministically', () => {
  const output = renderDocumentation(schema, meta, 'v1.2.3');

  assert.equal(output, renderDocumentation(schema, meta, 'v1.2.3'));
  assert.match(output, /\*\*goose version:\*\* <code>v1\.2\.3<\/code>/);
  assert.match(output, /\[<code>Target<\/code>\]\(#schema-target\)/);
  assert.match(output, /### <code>Target<\/code> \{#schema-target\}/);
  assert.match(output, /\[<code>Response<\/code>\]\(#schema-response\) & .*<code>kind<\/code>/);
  assert.match(output, /Array&lt;<code>string<\/code>&gt; \\| <code>null<\/code>/);
  assert.match(output, /Record&lt;<code>string<\/code>, <code>integer<\/code>&gt;/);
  assert.match(output, /Default: <code>"safe"<\/code>/);
  assert.match(output, /Minimum: <code>1<\/code><br \/>Maximum: <code>10<\/code>/);
  assert.match(output, /Format: <code>slug<\/code><br \/>Pattern: <code>\^\[a-z\]\+\$<\/code>/);
});

test('rejects unsupported schema shapes', () => {
  assert.throws(() => schemaType({not: {type: 'string'}}), /Unsupported schema keyword: not/);
});
