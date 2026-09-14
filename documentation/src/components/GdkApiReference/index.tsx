import React, { useContext, useEffect, useMemo, useState } from "react";
import clsx from "clsx";
import Link from "@docusaurus/Link";
import useBrokenLinks from "@docusaurus/useBrokenLinks";
import { useHistory, useLocation } from "@docusaurus/router";
import { useAnchorTargetClassName } from "@docusaurus/theme-common";
import CodeBlock from "@theme/CodeBlock";
import apiData from "@site/src/data/gdk-api.json";
import { LANGUAGES, Language, LanguageId } from "./languages";
import styles from "./styles.module.css";

type GdkParam = {
  name: string;
  type: string;
  default: string | null;
  docs: string;
};

type GdkFunc = {
  name: string;
  docs: string;
  params: GdkParam[];
  returns: string | null;
  throws: string | null;
  isAsync: boolean;
};

type GdkItem = {
  name: string;
  kind: "object" | "callback" | "record" | "enum" | "error";
  docs: string;
  fields: GdkParam[];
  variants: { name: string; fields: GdkParam[] }[];
  methods: GdkFunc[];
};

type GdkApiDoc = {
  version: string;
  docVersion: string;
  source: string;
  functions: GdkFunc[];
  items: GdkItem[];
};

const VERSIONS = (apiData as { versions: GdkApiDoc[] }).versions;

const DEFAULT_VERSION = VERSIONS[0].docVersion;
const DEFAULT_LANGUAGE = LANGUAGES[0].id;
const VERSION_PARAM = "version";
const LANGUAGE_PARAM = "language";

type Selection = { docVersion: string; languageId: LanguageId };

const isKnownVersion = (docVersion: string | null): docVersion is string =>
  VERSIONS.some((entry) => entry.docVersion === docVersion);

const isKnownLanguage = (languageId: string | null): languageId is LanguageId =>
  LANGUAGES.some((entry) => entry.id === languageId);

// Links carry the reader's version and language so a shared anchor lands on the
// same content. Params missing from older links fall back to the defaults.
const readSelection = (search: string): Selection => {
  const params = new URLSearchParams(search);
  const requestedVersion = params.get(VERSION_PARAM);
  const requestedLanguage = params.get(LANGUAGE_PARAM);
  return {
    docVersion: isKnownVersion(requestedVersion) ? requestedVersion : DEFAULT_VERSION,
    languageId: isKnownLanguage(requestedLanguage) ? requestedLanguage : DEFAULT_LANGUAGE,
  };
};

const selectionSearch = (selection: Selection, search: string) => {
  const params = new URLSearchParams(search);
  params.set(VERSION_PARAM, selection.docVersion);
  params.set(LANGUAGE_PARAM, selection.languageId);
  return `?${params.toString()}`;
};

const SelectionSearchContext = React.createContext(
  `?${VERSION_PARAM}=${DEFAULT_VERSION}&${LANGUAGE_PARAM}=${DEFAULT_LANGUAGE}`,
);

const KIND_LABELS: Record<GdkItem["kind"], string> = {
  object: "Class",
  callback: "Interface",
  record: "Data type",
  enum: "Enum",
  error: "Error",
};

const KIND_HEADINGS: Record<GdkItem["kind"], string> = {
  object: "Classes",
  callback: "Interfaces",
  record: "Data types",
  enum: "Enums",
  error: "Errors",
};

const slug = (...parts: string[]) =>
  parts.join("-").replace(/[^a-zA-Z0-9]+/g, "-").toLowerCase();

// Anchors use the canonical Rust names so a link keeps working when the reader
// switches the language or version toggle.
const funcAnchor = (func: GdkFunc, owner?: string) => slug(owner ?? "fn", func.name);
const itemAnchor = (item: GdkItem) => slug(item.name);
const memberAnchor = (ownerAnchor: string, kind: string, name: string) =>
  slug(ownerAnchor, kind, name);

function anchorsForFunc(func: GdkFunc, owner?: string): string[] {
  const anchor = funcAnchor(func, owner);
  return [anchor, ...func.params.map((param) => memberAnchor(anchor, "param", param.name))];
}

function anchorsForDoc(doc: GdkApiDoc): string[] {
  const anchors = ["functions"];
  doc.functions.forEach((func) => anchors.push(...anchorsForFunc(func)));

  doc.items.forEach((item) => {
    const anchor = itemAnchor(item);
    anchors.push(anchor, slug(item.kind, "types"));
    item.fields.forEach((field) => anchors.push(memberAnchor(anchor, "field", field.name)));
    item.variants.forEach((variant) =>
      anchors.push(memberAnchor(anchor, "variant", variant.name)),
    );
    item.methods.forEach((method) => anchors.push(...anchorsForFunc(method, item.name)));
  });

  return anchors;
}

function signature(func: GdkFunc, language: Language, owner?: string): string {
  const params = func.params
    .map((param) => {
      const type = language.type(param.type);
      const suffix = param.default ? ` = ${language.default(param.default)}` : "";
      switch (language.id) {
        case "rust":
          return `${param.name}: ${type}${suffix}`;
        case "python":
          return `${language.field(param.name)}: ${type}${suffix}`;
        default:
          return `${language.field(param.name)}: ${type}${suffix}`;
      }
    })
    .join(", ");

  const name = language.func(func.name);
  const returns = func.returns ? language.type(func.returns) : null;
  const prefix = owner ? `${owner}.` : "";

  if (language.id === "rust") {
    const asyncKeyword = func.isAsync ? "async " : "";
    const result = func.throws
      ? `Result<${returns ?? "()"}, ${func.throws}>`
      : returns;
    return `${asyncKeyword}fn ${prefix}${name}(${params})${result ? ` -> ${result}` : ""}`;
  }

  if (language.id === "python") {
    const asyncKeyword = func.isAsync ? "async " : "";
    return `${asyncKeyword}def ${prefix}${name}(${params})${returns ? ` -> ${returns}` : ""}`;
  }

  const suspend = func.isAsync ? "suspend " : "";
  const throwsAnnotation = func.throws
    ? `@Throws(${language.errorType(func.throws)}::class)\n`
    : "";
  return `${throwsAnnotation}${suspend}fun ${prefix}${name}(${params})${returns ? `: ${returns}` : ""}`;
}

function HashLink({ anchor, label }: { anchor: string; label: string }) {
  const search = useContext(SelectionSearchContext);
  const title = `Direct link to ${label}`;
  return (
    <Link
      className="hash-link"
      to={`${search}#${anchor}`}
      aria-label={title}
      title={title}
      translate="no"
    >
      &#8203;
    </Link>
  );
}

function Anchored({
  as: As,
  anchor,
  label,
  className,
  children,
}: {
  as: "h2" | "h3" | "h4" | "td";
  anchor: string;
  label: string;
  className?: string;
  children: React.ReactNode;
}) {
  const anchorTargetClassName = useAnchorTargetClassName(anchor);
  return (
    <As id={anchor} className={clsx("anchor", anchorTargetClassName, className)}>
      {children}
      <HashLink anchor={anchor} label={label} />
    </As>
  );
}

function ParamTable({
  rows,
  language,
  caption,
  ownerAnchor,
  rowKind,
}: {
  rows: GdkParam[];
  language: Language;
  caption: string;
  ownerAnchor: string;
  rowKind: string;
}) {
  if (rows.length === 0) return null;
  const hasDefaults = rows.some((row) => row.default);
  return (
    <table className={styles.table}>
      <thead>
        <tr>
          <th>{caption}</th>
          <th>Type</th>
          {hasDefaults && <th>Default</th>}
          <th>Description</th>
        </tr>
      </thead>
      <tbody>
        {rows.map((row) => {
          const name = language.field(row.name);
          return (
            <tr key={row.name}>
              <Anchored
                as="td"
                anchor={memberAnchor(ownerAnchor, rowKind, row.name)}
                label={name}
                className={styles.nameCell}
              >
                <code>{name}</code>
              </Anchored>
              <td>
                <code>{language.type(row.type)}</code>
              </td>
              {hasDefaults && (
                <td>{row.default ? <code>{language.default(row.default)}</code> : "—"}</td>
              )}
              <td>{row.docs || "—"}</td>
            </tr>
          );
        })}
      </tbody>
    </table>
  );
}

function FuncEntry({
  func,
  language,
  owner,
}: {
  func: GdkFunc;
  language: Language;
  owner?: string;
}) {
  const anchor = funcAnchor(func, owner);
  const name = language.func(func.name);
  return (
    <div className={styles.entry}>
      <Anchored as="h4" anchor={anchor} label={name} className={styles.entryTitle}>
        <code>{name}</code>
      </Anchored>
      {func.docs && <p>{func.docs}</p>}
      <CodeBlock language={language.prism}>{signature(func, language, owner)}</CodeBlock>
      <ParamTable
        rows={func.params}
        language={language}
        caption="Parameter"
        ownerAnchor={anchor}
        rowKind="param"
      />
      {func.throws && (
        <p className={styles.meta}>
          Raises <code>{language.errorType(func.throws)}</code>
        </p>
      )}
    </div>
  );
}

function ItemEntry({ item, language }: { item: GdkItem; language: Language }) {
  const dataCarrying = item.variants.some((variant) => variant.fields.length > 0);
  const anchor = itemAnchor(item);
  const name = item.kind === "error" ? language.errorType(item.name) : item.name;
  return (
    <section className={styles.item}>
      <Anchored as="h3" anchor={anchor} label={name} className={styles.itemTitle}>
        <code>{name}</code>
        <span className={styles.badge}>{KIND_LABELS[item.kind]}</span>
      </Anchored>
      {item.docs && <p>{item.docs}</p>}

      <ParamTable
        rows={item.fields}
        language={language}
        caption="Field"
        ownerAnchor={anchor}
        rowKind="field"
      />

      {item.variants.length > 0 && (
        <table className={styles.table}>
          <thead>
            <tr>
              <th>{item.kind === "error" ? "Variant" : "Case"}</th>
              <th>Associated data</th>
            </tr>
          </thead>
          <tbody>
            {item.variants.map((variant) => {
              const variantName =
                item.kind === "error" && language.id === "kotlin"
                  ? `${language.errorType(item.name)}.${variant.name}`
                  : language.variant(variant.name, dataCarrying);
              return (
                <tr key={variant.name}>
                  <Anchored
                    as="td"
                    anchor={memberAnchor(anchor, "variant", variant.name)}
                    label={variantName}
                    className={styles.nameCell}
                  >
                    <code>{variantName}</code>
                  </Anchored>
                  <td>
                    {variant.fields.length === 0
                      ? "—"
                      : variant.fields.map((field) => (
                          <div key={field.name}>
                            <code>
                              {language.field(field.name)}: {language.type(field.type)}
                            </code>
                          </div>
                        ))}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}

      {item.methods.map((method) => (
        <FuncEntry key={method.name} func={method} language={language} owner={item.name} />
      ))}
    </section>
  );
}

export default function GdkApiReference() {
  const [selection, setSelection] = useState<Selection>({
    docVersion: DEFAULT_VERSION,
    languageId: DEFAULT_LANGUAGE,
  });
  const { docVersion, languageId } = selection;
  const brokenLinks = useBrokenLinks();
  const history = useHistory();
  const location = useLocation();

  useEffect(() => {
    const requested = readSelection(location.search);
    setSelection(requested);

    const canonical = selectionSearch(requested, location.search);
    if (canonical !== `?${new URLSearchParams(location.search)}`) {
      history.replace({ search: canonical, hash: location.hash });
    }
  }, [history, location.hash, location.search]);

  const select = (next: Partial<Selection>) =>
    history.replace({
      search: selectionSearch({ ...selection, ...next }, location.search),
      hash: location.hash,
    });

  const language = LANGUAGES.find((entry) => entry.id === languageId)!;
  const doc = useMemo(
    () => VERSIONS.find((entry) => entry.docVersion === docVersion) ?? VERSIONS[0],
    [docVersion],
  );

  anchorsForDoc(VERSIONS[0]).forEach((anchor) => brokenLinks.collectAnchor(anchor));

  useEffect(() => {
    const anchor = location.hash.slice(1);
    if (!anchor) return;
    document.getElementById(decodeURIComponent(anchor))?.scrollIntoView();
  }, [docVersion, languageId, location.hash]);

  const grouped = useMemo(() => {
    const order: GdkItem["kind"][] = ["object", "callback", "record", "enum", "error"];
    return order
      .map((kind) => ({ kind, items: doc.items.filter((item) => item.kind === kind) }))
      .filter((group) => group.items.length > 0);
  }, [doc]);

  return (
    <SelectionSearchContext.Provider value={selectionSearch(selection, location.search)}>
      <div>
        <div className={styles.toolbar}>
          <div className={styles.tabs} role="tablist" aria-label="SDK language">
            {LANGUAGES.map((entry) => (
              <button
                key={entry.id}
                type="button"
                role="tab"
                aria-selected={entry.id === languageId}
                className={entry.id === languageId ? styles.tabActive : styles.tab}
                onClick={() => select({ languageId: entry.id })}
              >
                {entry.label}
              </button>
            ))}
          </div>

          <label className={styles.version}>
            Version
            <select
              value={docVersion}
              onChange={(event) => select({ docVersion: event.target.value })}
              aria-label="SDK version"
            >
              {VERSIONS.map((entry) => (
                <option key={entry.docVersion} value={entry.docVersion}>
                  {entry.docVersion}.x
                </option>
              ))}
            </select>
          </label>
        </div>

        <p className={styles.meta}>
          Generated from <code>{doc.source}</code> at <code>goose-sdk {doc.version}</code>.
        </p>

        <Anchored as="h2" anchor="functions" label="Functions">
          Functions
        </Anchored>
        {doc.functions.map((func) => (
          <FuncEntry key={func.name} func={func} language={language} />
        ))}

        {grouped.map((group) => (
          <React.Fragment key={group.kind}>
            <Anchored
              as="h2"
              anchor={slug(group.kind, "types")}
              label={KIND_HEADINGS[group.kind]}
            >
              {KIND_HEADINGS[group.kind]}
            </Anchored>
            {group.items.map((item) => (
              <ItemEntry key={item.name} item={item} language={language} />
            ))}
          </React.Fragment>
        ))}
      </div>
    </SelectionSearchContext.Provider>
  );
}
