#!/usr/bin/env node
/**
 * Generates TypeScript types + Zod validators for Goose custom extension methods.
 *
 * Usage:
 *   npm run generate              # build Rust schema, then generate TS
 */

import { createClient } from "@hey-api/openapi-ts";
import * as fs from "fs/promises";
import { dirname, resolve } from "path";
import { fileURLToPath } from "url";
import * as prettier from "prettier";

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const ROOT = resolve(__dirname, "../..");
const SCHEMA_PATH = resolve(ROOT, "crates/goose/acp-schema.json");
const META_PATH = resolve(ROOT, "crates/goose/acp-meta.json");
const OUTPUT_DIR = resolve(__dirname, "src/generated");

// Export the main function so it can be imported by build-schema.ts
export default async function main() {
  const schemaSrc = await fs.readFile(SCHEMA_PATH, "utf8");
  const jsonSchema = JSON.parse(
    schemaSrc.replaceAll("#/$defs/", "#/components/schemas/"),
  );

  const metaSrc = await fs.readFile(META_PATH, "utf8");
  const meta = JSON.parse(metaSrc);

  await createClient({
    input: {
      openapi: "3.1.0",
      info: {
        title: "Goose Extensions",
        version: "1.0.0",
      },
      components: {
        schemas: jsonSchema.$defs,
      },
    },
    output: {
      path: OUTPUT_DIR,
    },
    plugins: [
      {
        case: "preserve",
        name: "zod",
      },
      {
        case: "preserve",
        name: "@hey-api/typescript",
      },
    ],
  });

  await postProcessTypes();
  await postProcessIndex(meta);

  await generateClient(meta);

  console.log(`\nGenerated Goose extension schema in ${OUTPUT_DIR}`);
}

async function postProcessTypes() {
  const tsPath = resolve(OUTPUT_DIR, "types.gen.ts");
  let src = await fs.readFile(tsPath, "utf8");
  src = src.replace(/\nexport type ClientOptions =[\s\S]*?^};\n/m, "\n");
  await fs.writeFile(tsPath, src);
}

async function postProcessIndex(meta: {
  methods: unknown[];
  notifications?: unknown[];
  agentRequests?: unknown[];
}) {
  const indexPath = resolve(OUTPUT_DIR, "index.ts");
  let src = await fs.readFile(indexPath, "utf8");

  src = src.replace(/,?\s*ClientOptions\s*,?/g, (match) => {
    if (match.startsWith(",") && match.endsWith(",")) return ",";
    if (match.startsWith(",")) return "";
    return "";
  });

  src = fixRelativeImports(src);

  const methodConstants = await prettier.format(
    `
export const GOOSE_EXT_METHODS = ${JSON.stringify(meta.methods, null, 2)} as const;

export type GooseExtMethod = (typeof GOOSE_EXT_METHODS)[number];

export const GOOSE_EXT_NOTIFICATIONS = ${JSON.stringify(meta.notifications ?? [], null, 2)} as const;

export type GooseExtNotification = (typeof GOOSE_EXT_NOTIFICATIONS)[number];

export const GOOSE_EXT_AGENT_REQUESTS = ${JSON.stringify(meta.agentRequests ?? [], null, 2)} as const;

export type GooseExtAgentRequest = (typeof GOOSE_EXT_AGENT_REQUESTS)[number];
`,
    { parser: "typescript" },
  );

  await fs.writeFile(indexPath, `${src}\n${methodConstants}`);

  for (const file of ["zod.gen.ts", "types.gen.ts"]) {
    const filePath = resolve(OUTPUT_DIR, file);
    try {
      const content = await fs.readFile(filePath, "utf8");
      const fixed = fixRelativeImports(content);
      if (fixed !== content) {
        await fs.writeFile(filePath, fixed);
      }
    } catch {
      // File may not exist
    }
  }
}

function fixRelativeImports(src: string): string {
  return src.replace(
    /from\s+['"](\.[^'"]+)['"]/g,
    (_match, importPath: string) => {
      if (importPath.endsWith(".js") || importPath.endsWith(".json")) {
        return `from '${importPath}'`;
      }
      return `from '${importPath}.js'`;
    },
  );
}

interface MethodMeta {
  method: string;
  requestType: string | null;
  responseType: string | null;
}

function methodToCamelCase(method: string): string {
  let methodParts = method.split(/[/_]/).filter((part) => part.length > 0);

  let suffix: string;
  if (methodParts[0] == "goose" && methodParts[1] == "unstable") {
    methodParts.shift();
    methodParts.shift();
    suffix = "_unstable";
  } else {
    suffix = "";
  }

  let prefix = methodParts
    .map((part) =>
      part.replace(/[^a-zA-Z0-9]+(.)/g, (_, chr: string) => chr.toUpperCase()),
    )
    .map((part, i) =>
      i === 0 ? part : part.charAt(0).toUpperCase() + part.slice(1),
    )
    .join("");

  return `${prefix}${suffix}`;
}

async function generateClient(meta: { methods: MethodMeta[] }) {
  const typeImports = new Set<string>();
  const zodImports = new Set<string>();
  const upstreamTypeImports = new Set<string>(["ClientContext"]);

  const methodDefs: string[] = [];

  for (const m of meta.methods) {
    const fnName = methodToCamelCase(m.method);
    const fullMethod = m.method;

    let paramType = "";
    let paramArg = "";
    let callParams = "{}";
    if (m.requestType) {
      typeImports.add(m.requestType);
      paramType = m.requestType;
      paramArg = `params: ${paramType}`;
      callParams = "params";
    }

    let returnType: string;
    let bodyLines: string[];

    if (m.responseType && m.responseType !== "EmptyResponse") {
      typeImports.add(m.responseType);
      const zodName = `z${m.responseType}`;
      zodImports.add(zodName);
      returnType = m.responseType;
      bodyLines = [
        `const raw = await this.conn.request("${fullMethod}", ${callParams});`,
        `return ${zodName}.parse(raw) as ${returnType};`,
      ];
    } else if (m.responseType === "EmptyResponse") {
      returnType = "void";
      bodyLines = [`await this.conn.request("${fullMethod}", ${callParams});`];
    } else {
      returnType = "Record<string, unknown>";
      bodyLines = [
        `return await this.conn.request<Record<string, unknown>>("${fullMethod}", ${callParams ? callParams : "{}"});`,
      ];
    }

    methodDefs.push(`
  async ${fnName}(${paramArg}): Promise<${returnType}> {
    ${bodyLines.join("\n    ")}
  }`);
  }

  const upstreamImportLine = `import type { ${[...upstreamTypeImports].sort().join(", ")} } from "@agentclientprotocol/sdk";`;
  const typeImportLine = typeImports.size
    ? `import type { ${[...typeImports].sort().join(", ")} } from "./types.gen.js";`
    : "";
  const zodImportLine = zodImports.size
    ? `import { ${[...zodImports].sort().join(", ")} } from "./zod.gen.js";`
    : "";

  let src = `// This file is auto-generated — do not edit manually.

${upstreamImportLine}
${typeImportLine}
${zodImportLine}

export class GooseExtClient {
  constructor(private conn: Pick<ClientContext, "request">) {}
${methodDefs.join("\n")}
}
`;

  src = await prettier.format(src, { parser: "typescript" });
  src = fixRelativeImports(src);

  const clientPath = resolve(OUTPUT_DIR, "client.gen.ts");
  await fs.writeFile(clientPath, src);
}

// Run main if this file is executed directly
if (import.meta.url === `file://${process.argv[1]}`) {
  main().catch((err) => {
    console.error(err);
    process.exit(1);
  });
}
