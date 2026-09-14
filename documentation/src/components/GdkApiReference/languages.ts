// Renders the Rust API surface in each target language's idioms. The rules
// mirror the uniffi 0.32 code generators, which are the actual source of the
// Python and Kotlin bindings.

export type LanguageId = "rust" | "python" | "kotlin";

const toSnake = (name: string) => name;
const toCamel = (name: string) =>
  name.replace(/_([a-z0-9])/g, (_, char: string) => char.toUpperCase());
const toShoutySnake = (name: string) =>
  name
    .replace(/([a-z0-9])([A-Z])/g, "$1_$2")
    .replace(/([A-Z]+)([A-Z][a-z])/g, "$1_$2")
    .toUpperCase();

type Scalars = Record<string, string>;

const PYTHON_SCALARS: Scalars = {
  String: "str",
  bool: "bool",
  i8: "int",
  i16: "int",
  i32: "int",
  i64: "int",
  u8: "int",
  u16: "int",
  u32: "int",
  u64: "int",
  f32: "float",
  f64: "float",
  "()": "None",
};

const KOTLIN_SCALARS: Scalars = {
  String: "String",
  bool: "Boolean",
  i8: "Byte",
  i16: "Short",
  i32: "Int",
  i64: "Long",
  u8: "UByte",
  u16: "UShort",
  u32: "UInt",
  u64: "ULong",
  f32: "Float",
  f64: "Double",
  "()": "Unit",
};

const generic = (type: string, name: string): string[] | null => {
  const match = new RegExp(`^${name}\\s*<(.+)>$`, "s").exec(type.trim());
  if (!match) return null;
  const args: string[] = [];
  let depth = 0;
  let current = "";
  for (const char of match[1]) {
    if (char === "<") depth += 1;
    if (char === ">") depth -= 1;
    if (char === "," && depth === 0) {
      args.push(current.trim());
      current = "";
    } else {
      current += char;
    }
  }
  if (current.trim()) args.push(current.trim());
  return args;
};

const mapType = (type: string, language: LanguageId): string => {
  const trimmed = type.trim();
  if (language === "rust") return trimmed;

  const scalars = language === "python" ? PYTHON_SCALARS : KOTLIN_SCALARS;
  if (scalars[trimmed]) return scalars[trimmed];

  const option = generic(trimmed, "Option");
  if (option) {
    const inner = mapType(option[0], language);
    return language === "python" ? `${inner} | None` : `${inner}?`;
  }

  const bytes = generic(trimmed, "Vec");
  if (bytes && bytes[0].trim() === "u8") {
    return language === "python" ? "bytes" : "ByteArray";
  }
  if (bytes) {
    const inner = mapType(bytes[0], language);
    return language === "python" ? `list[${inner}]` : `List<${inner}>`;
  }

  const map = generic(trimmed, "HashMap");
  if (map) {
    const [key, value] = map.map((arg) => mapType(arg, language));
    return language === "python" ? `dict[${key}, ${value}]` : `Map<${key}, ${value}>`;
  }

  return trimmed;
};

const mapDefault = (value: string, language: LanguageId): string => {
  if (language === "rust") return value;
  if (value === "None") return language === "python" ? "None" : "null";
  if (value === "true" || value === "false") {
    return language === "python" ? (value === "true" ? "True" : "False") : value;
  }
  return value;
};

export type Language = {
  id: LanguageId;
  label: string;
  /** Prism language for syntax highlighting. */
  prism: string;
  func: (name: string) => string;
  field: (name: string) => string;
  variant: (name: string, isDataCarrying: boolean) => string;
  type: (type: string) => string;
  default: (value: string) => string;
  errorType: (name: string) => string;
};

export const LANGUAGES: Language[] = [
  {
    id: "rust",
    label: "Rust",
    prism: "rust",
    func: toSnake,
    field: toSnake,
    variant: (name) => name,
    type: (type) => mapType(type, "rust"),
    default: (value) => mapDefault(value, "rust"),
    errorType: (name) => name,
  },
  {
    id: "python",
    label: "Python",
    prism: "python",
    func: toSnake,
    field: toSnake,
    // Flat enums become `enum.Enum` members; data-carrying variants become
    // nested dataclasses that keep their Rust casing.
    variant: (name, isDataCarrying) => (isDataCarrying ? name : toShoutySnake(name)),
    type: (type) => mapType(type, "python"),
    default: (value) => mapDefault(value, "python"),
    errorType: (name) => name,
  },
  {
    id: "kotlin",
    label: "Kotlin",
    prism: "kotlin",
    func: toCamel,
    field: toCamel,
    // Flat enums become `enum class` entries; data-carrying variants become
    // `sealed class` subclasses that keep their Rust casing.
    variant: (name, isDataCarrying) => (isDataCarrying ? name : toShoutySnake(name)),
    type: (type) => mapType(type, "kotlin"),
    default: (value) => mapDefault(value, "kotlin"),
    errorType: (name) => name.replace(/Error$/, "Exception"),
  },
];
