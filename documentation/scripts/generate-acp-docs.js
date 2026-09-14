const fs = require('fs');
const path = require('path');

const ROOT = path.join(__dirname, '..', '..');
const SCHEMA_FILE = path.join(ROOT, 'crates', 'goose', 'acp-schema.json');
const META_FILE = path.join(ROOT, 'crates', 'goose', 'acp-meta.json');
const OUTPUT_FILE = path.join(
  ROOT,
  'documentation',
  'docs',
  'gdk',
  'acp',
  'reference.md'
);
const UNSUPPORTED_KEYWORDS = [
  'contains',
  'dependentSchemas',
  'else',
  'if',
  'not',
  'patternProperties',
  'prefixItems',
  'propertyNames',
  'then',
  'unevaluatedProperties',
];

function escapeCode(value) {
  return String(value)
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('{', '&#123;')
    .replaceAll('}', '&#125;')
    .replaceAll('`', '&#96;');
}

function code(value) {
  return `<code>${escapeCode(value)}</code>`;
}

function methodCode(value) {
  return `<code>${escapeCode(value).replaceAll('/', '/<wbr />')}</code>`;
}

function schemaCode(value) {
  const escaped = escapeCode(value)
    .replace(/([a-z0-9])([A-Z])/g, '$1<wbr />$2')
    .replaceAll('_', '_<wbr />');
  return `<code>${escaped}</code>`;
}

function text(value, fallback = '') {
  const result = String(value ?? '').trim();
  return result
    ? result.replaceAll('|', '\\|').replaceAll('\r\n', '\n').replaceAll('\n', '<br />')
    : fallback;
}

function literal(value) {
  return code(JSON.stringify(value));
}

function refName(reference) {
  const prefix = '#/$defs/';
  if (typeof reference !== 'string' || !reference.startsWith(prefix)) {
    throw new Error(`Unsupported schema reference: ${JSON.stringify(reference)}`);
  }
  return reference.slice(prefix.length);
}

function schemaId(name) {
  return `schema-${name.toLowerCase()}`;
}

function schemaLink(name) {
  return `[${schemaCode(name)}](#${schemaId(name)})`;
}

function schemaType(schema, inlineObject = false) {
  if (schema === true) return code('unknown');
  if (schema === false) return code('never');
  if (!schema || Array.isArray(schema) || typeof schema !== 'object') {
    throw new Error(`Unsupported schema: ${JSON.stringify(schema)}`);
  }

  const unsupported = UNSUPPORTED_KEYWORDS.find((keyword) => keyword in schema);
  if (unsupported) throw new Error(`Unsupported schema keyword: ${unsupported}`);

  if (schema.$ref) {
    const structuralSiblings = Object.fromEntries(
      Object.entries(schema).filter(
        ([key]) =>
          ![
            '$ref',
            'default',
            'description',
            'format',
            'maximum',
            'minimum',
            'pattern',
            'title',
          ].includes(key) && !key.startsWith('x-')
      )
    );
    const reference = schemaLink(refName(schema.$ref));
    return Object.keys(structuralSiblings).length === 0
      ? reference
      : `${reference} & ${schemaType(structuralSiblings, true)}`;
  }
  if (Object.hasOwn(schema, 'const')) return literal(schema.const);
  if (schema.enum) return schema.enum.map(literal).join(' \\| ');

  for (const keyword of ['anyOf', 'oneOf', 'allOf']) {
    if (schema[keyword]) {
      if (!Array.isArray(schema[keyword]) || schema[keyword].length === 0) {
        throw new Error(`Invalid ${keyword} schema`);
      }
      const separator = keyword === 'allOf' ? ' & ' : ' \\| ';
      return schema[keyword].map((part) => schemaType(part, true)).join(separator);
    }
  }

  if (Array.isArray(schema.type)) {
    return schema.type
      .map((type) => schemaType({...schema, type}, inlineObject))
      .join(' \\| ');
  }

  if (schema.type === 'array') {
    if (!Object.hasOwn(schema, 'items')) throw new Error('Array schema is missing items');
    return `Array&lt;${schemaType(schema.items, true)}&gt;`;
  }

  if (
    schema.type === 'object' ||
    (!schema.type && (schema.properties || Object.hasOwn(schema, 'additionalProperties')))
  ) {
    if (schema.properties && inlineObject) {
      const required = new Set(schema.required ?? []);
      const fields = Object.entries(schema.properties).map(
        ([name, property]) =>
          `${code(name)}${required.has(name) ? '' : '?'}: ${schemaType(property, true)}`
      );
      return `&#123; ${fields.join('; ')} &#125;`;
    }
    if (Object.hasOwn(schema, 'additionalProperties') && !schema.properties) {
      return `Record&lt;${code('string')}, ${schemaType(schema.additionalProperties, true)}&gt;`;
    }
    return code('object');
  }

  if (typeof schema.type === 'string') return code(schema.type);
  if (
    Object.keys(schema).every(
      (key) =>
        key === 'default' || key === 'description' || key === 'title' || key.startsWith('x-')
    )
  ) {
    return code('unknown');
  }
  throw new Error(`Unsupported schema shape: ${JSON.stringify(schema)}`);
}

function constraints(schema) {
  const values = [];
  if (Object.hasOwn(schema, 'default')) values.push(`Default: ${literal(schema.default)}`);
  if (schema.format !== undefined) values.push(`Format: ${code(schema.format)}`);
  if (schema.minimum !== undefined) values.push(`Minimum: ${code(schema.minimum)}`);
  if (schema.maximum !== undefined) values.push(`Maximum: ${code(schema.maximum)}`);
  if (schema.pattern !== undefined) values.push(`Pattern: ${code(schema.pattern)}`);
  return values.join('<br />');
}

function renderProperties(schema) {
  const properties = Object.entries(schema.properties ?? {});
  if (properties.length === 0) return '';

  const required = new Set(schema.required ?? []);
  const rows = properties.map(([name, property]) =>
    `| ${code(name)} | ${schemaType(property)} | ${required.has(name) ? 'Yes' : 'No'} | ${text(property.description, '—')} | ${constraints(property) || '—'} |`
  );
  return [
    '| Field | Type | Required | Description | Constraints |',
    '|---|---|:---:|---|---|',
    ...rows,
    '',
  ].join('\n');
}

function renderMethods(entries, schemas, notification = false) {
  if (entries.length === 0) return '_None._\n';

  const rows = [...entries]
    .sort((left, right) => left.method.localeCompare(right.method))
    .map((entry) => {
      const inputType = notification ? entry.paramsType : entry.requestType;
      const inputSchema = schemas.$defs[inputType];
      if (!inputSchema) {
        throw new Error(`${entry.method} references missing type ${inputType}`);
      }
      if (!notification && !schemas.$defs[entry.responseType]) {
        throw new Error(`${entry.method} references missing type ${entry.responseType}`);
      }
      const links = notification
        ? `**Parameters:** ${schemaLink(inputType)}`
        : `**Request:** ${schemaLink(inputType)}<br />**Response:** ${schemaLink(entry.responseType)}`;
      const description = text(inputSchema.description);
      const method = description
        ? `${methodCode(entry.method)}<br />${description}`
        : methodCode(entry.method);
      return `| ${method} | ${links} |`;
    });

  return ['| Method | Schemas |', '|---|---|', ...rows, ''].join('\n');
}

function renderSchema(name, schema) {
  const lines = [`### ${code(name)} {#${schemaId(name)}}`, ''];
  if (schema.description) lines.push(text(schema.description), '');
  lines.push(`**Type:** ${schemaType(schema)}`, '');

  const schemaConstraints = constraints(schema);
  if (schemaConstraints) lines.push(`**Constraints:** ${schemaConstraints}`, '');

  const properties = renderProperties(schema);
  if (properties) lines.push(properties);
  return lines.join('\n');
}

function renderDocumentation(schemas, meta, gooseVersion = 'Preview') {
  if (!schemas.$defs || typeof schemas.$defs !== 'object') {
    throw new Error('Schema document is missing $defs');
  }

  const definitions = Object.entries(schemas.$defs)
    .filter(([, schema]) => !schema['x-docs-ignore'])
    .sort(([left], [right]) => left.localeCompare(right));

  const output = [
    '---',
    'title: goose ACP Reference',
    'sidebar_label: Reference',
    'sidebar_position: 2',
    '---',
    '',
    '# goose ACP Reference',
    '',
    'This reference documents goose-specific Agent Client Protocol methods. Standard ACP methods are documented by the [Agent Client Protocol specification](https://agentclientprotocol.com/).',
    '',
    `**goose version:** ${code(gooseVersion)}`,
    '',
    '> This file is generated from `crates/goose/acp-schema.json` and `crates/goose/acp-meta.json`. Do not edit it manually.',
    '',
    '## Client-to-agent requests',
    '',
    renderMethods(meta.methods ?? [], schemas),
    '## Agent-to-client requests',
    '',
    renderMethods(meta.agentRequests ?? [], schemas),
    '## Agent-to-client notifications',
    '',
    renderMethods(meta.notifications ?? [], schemas, true),
    '## Schemas',
    '',
    ...definitions.map(([name, schema]) => renderSchema(name, schema)),
  ];
  return `${output.join('\n').trimEnd()}\n`;
}

function main() {
  const args = process.argv.slice(2);
  if (args.length !== 0 && args.length !== 3 && args.length !== 4) {
    throw new Error(
      'Usage: generate-acp-docs.js [schema-file meta-file output-file [goose-version]]'
    );
  }

  const [
    schemaFile = SCHEMA_FILE,
    metaFile = META_FILE,
    outputFile = OUTPUT_FILE,
    gooseVersion = 'Preview',
  ] = args;
  const schemas = JSON.parse(fs.readFileSync(schemaFile, 'utf8'));
  const meta = JSON.parse(fs.readFileSync(metaFile, 'utf8'));
  const output = renderDocumentation(schemas, meta, gooseVersion);
  fs.mkdirSync(path.dirname(outputFile), {recursive: true});
  fs.writeFileSync(outputFile, output);
  console.log(`[generate-acp-docs] Generated: ${outputFile}`);
}

if (require.main === module) main();

module.exports = {renderDocumentation, schemaType};
