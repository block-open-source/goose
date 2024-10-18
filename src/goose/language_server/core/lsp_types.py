# Code generated. DO NOT EDIT.
# LSP v3.17.0
# TODO: Look into use of https://pypi.org/project/ts2python/ to generate the types for https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/

"""
This file provides the Python types corresponding to the Typescript types defined in the language server protocol.
This file is obtained from https://github.com/microsoft/multilspy, which itself took it
from https://github.com/predragnikolic/OLSP under the MIT License with the following terms:

MIT License

Copyright (c) 2023 Предраг Николић

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
"""

from enum import Enum, IntEnum, IntFlag
from typing import Dict, List, Literal, Union, NotRequired, TypedDict

URI = str
DocumentUri = str
Uint = int
RegExp = str


class SemanticTokenTypes(Enum):
    """A set of predefined token types. This set is not fixed
    an clients can specify additional token types via the
    corresponding client capabilities.

    @since 3.16.0"""

    Namespace = "namespace"
    Type = "type"
    """ Represents a generic type. Acts as a fallback for types which can't be mapped to
    a specific type like class or enum. """
    Class = "class"
    Enum = "enum"
    Interface = "interface"
    Struct = "struct"
    TypeParameter = "typeParameter"
    Parameter = "parameter"
    Variable = "variable"
    Property = "property"
    EnumMember = "enumMember"
    Event = "event"
    Function = "function"
    Method = "method"
    Macro = "macro"
    Keyword = "keyword"
    Modifier = "modifier"
    Comment = "comment"
    String = "string"
    Number = "number"
    Regexp = "regexp"
    Operator = "operator"
    Decorator = "decorator"
    """ @since 3.17.0 """


class SemanticTokenModifiers(Enum):
    """A set of predefined token modifiers. This set is not fixed
    an clients can specify additional token types via the
    corresponding client capabilities.

    @since 3.16.0"""

    Declaration = "declaration"
    Definition = "definition"
    Readonly = "readonly"
    Static = "static"
    Deprecated = "deprecated"
    Abstract = "abstract"
    Async = "async"
    Modification = "modification"
    Documentation = "documentation"
    DefaultLibrary = "defaultLibrary"


class DocumentDiagnosticReportKind(Enum):
    """The document diagnostic report kinds.

    @since 3.17.0"""

    Full = "full"
    """ A diagnostic report with a full
    set of problems. """
    Unchanged = "unchanged"
    """ A report indicating that the last
    returned report is still accurate. """


class ErrorCodes(IntEnum):
    """Predefined error codes."""

    ParseError = -32700
    InvalidRequest = -32600
    MethodNotFound = -32601
    InvalidParams = -32602
    InternalError = -32603
    ServerNotInitialized = -32002
    """ Error code indicating that a server received a notification or
    request before the server has received the `initialize` request. """
    UnknownErrorCode = -32001


class LSPErrorCodes(IntEnum):
    RequestFailed = -32803
    """ A request failed but it was syntactically correct, e.g the
    method name was known and the parameters were valid. The error
    message should contain human readable information about why
    the request failed.

    @since 3.17.0 """
    ServerCancelled = -32802
    """ The server cancelled the request. This error code should
    only be used for requests that explicitly support being
    server cancellable.

    @since 3.17.0 """
    ContentModified = -32801
    """ The server detected that the content of a document got
    modified outside normal conditions. A server should
    NOT send this error code if it detects a content change
    in it unprocessed messages. The result even computed
    on an older state might still be useful for the client.

    If a client decides that a result is not of any use anymore
    the client should cancel the request. """
    RequestCancelled = -32800
    """ The client has canceled a request and a server as detected
    the cancel. """


class FoldingRangeKind(Enum):
    """A set of predefined range kinds."""

    Comment = "comment"
    """ Folding range for a comment """
    Imports = "imports"
    """ Folding range for an import or include """
    Region = "region"
    """ Folding range for a region (e.g. `#region`) """


class SymbolKind(IntEnum):
    """A symbol kind."""

    File = 1
    Module = 2
    Namespace = 3
    Package = 4
    Class = 5
    Method = 6
    Property = 7
    Field = 8
    Constructor = 9
    Enum = 10
    Interface = 11
    Function = 12
    Variable = 13
    Constant = 14
    String = 15
    Number = 16
    Boolean = 17
    Array = 18
    Object = 19
    Key = 20
    Null = 21
    EnumMember = 22
    Struct = 23
    Event = 24
    Operator = 25
    TypeParameter = 26


class SymbolTag(IntEnum):
    """Symbol tags are extra annotations that tweak the rendering of a symbol.

    @since 3.16"""

    Deprecated = 1
    """ Render a symbol as obsolete, usually using a strike-out. """


class UniquenessLevel(Enum):
    """Moniker uniqueness level to define scope of the moniker.

    @since 3.16.0"""

    Document = "document"
    """ The moniker is only unique inside a document """
    Project = "project"
    """ The moniker is unique inside a project for which a dump got created """
    Group = "group"
    """ The moniker is unique inside the group to which a project belongs """
    Scheme = "scheme"
    """ The moniker is unique inside the moniker scheme. """
    Global = "global"
    """ The moniker is globally unique """


class MonikerKind(Enum):
    """The moniker kind.

    @since 3.16.0"""

    Import = "import"
    """ The moniker represent a symbol that is imported into a project """
    Export = "export"
    """ The moniker represents a symbol that is exported from a project """
    Local = "local"
    """ The moniker represents a symbol that is local to a project (e.g. a local
    variable of a function, a class not visible outside the project, ...) """


class InlayHintKind(IntEnum):
    """Inlay hint kinds.

    @since 3.17.0"""

    Type = 1
    """ An inlay hint that for a type annotation. """
    Parameter = 2
    """ An inlay hint that is for a parameter. """


class MessageType(IntEnum):
    """The message type"""

    Error = 1
    """ An error message. """
    Warning = 2
    """ A warning message. """
    Info = 3
    """ An information message. """
    Log = 4
    """ A log message. """


class TextDocumentSyncKind(IntEnum):
    """Defines how the host (editor) should sync
    document changes to the language server."""

    None_ = 0
    """ Documents should not be synced at all. """
    Full = 1
    """ Documents are synced by always sending the full content
    of the document. """
    Incremental = 2
    """ Documents are synced by sending the full content on open.
    After that only incremental updates to the document are
    send. """


class TextDocumentSaveReason(IntEnum):
    """Represents reasons why a text document is saved."""

    Manual = 1
    """ Manually triggered, e.g. by the user pressing save, by starting debugging,
    or by an API call. """
    AfterDelay = 2
    """ Automatic after a delay. """
    FocusOut = 3
    """ When the editor lost focus. """


class CompletionItemKind(IntEnum):
    """The kind of a completion entry."""

    Text = 1
    Method = 2
    Function = 3
    Constructor = 4
    Field = 5
    Variable = 6
    Class = 7
    Interface = 8
    Module = 9
    Property = 10
    Unit = 11
    Value = 12
    Enum = 13
    Keyword = 14
    Snippet = 15
    Color = 16
    File = 17
    Reference = 18
    Folder = 19
    EnumMember = 20
    Constant = 21
    Struct = 22
    Event = 23
    Operator = 24
    TypeParameter = 25


class CompletionItemTag(IntEnum):
    """Completion item tags are extra annotations that tweak the rendering of a completion
    item.

    @since 3.15.0"""

    Deprecated = 1
    """ Render a completion as obsolete, usually using a strike-out. """


class InsertTextFormat(IntEnum):
    """Defines whether the insert text in a completion item should be interpreted as
    plain text or a snippet."""

    PlainText = 1
    """ The primary text to be inserted is treated as a plain string. """
    Snippet = 2
    """ The primary text to be inserted is treated as a snippet.

    A snippet can define tab stops and placeholders with `$1`, `$2`
    and `${3:foo}`. `$0` defines the final tab stop, it defaults to
    the end of the snippet. Placeholders with equal identifiers are linked,
    that is typing in one will update others too.

    See also: https://microsoft.github.io/language-server-protocol/specifications/specification-current/#snippet_syntax """


class InsertTextMode(IntEnum):
    """How whitespace and indentation is handled during completion
    item insertion.

    @since 3.16.0"""

    AsIs = 1
    """ The insertion or replace strings is taken as it is. If the
    value is multi line the lines below the cursor will be
    inserted using the indentation defined in the string value.
    The client will not apply any kind of adjustments to the
    string. """
    AdjustIndentation = 2
    """ The editor adjusts leading whitespace of new lines so that
    they match the indentation up to the cursor of the line for
    which the item is accepted.

    Consider a line like this: <2tabs><cursor><3tabs>foo. Accepting a
    multi line completion item is indented using 2 tabs and all
    following lines inserted will be indented using 2 tabs as well. """


class DocumentHighlightKind(IntEnum):
    """A document highlight kind."""

    Text = 1
    """ A textual occurrence. """
    Read = 2
    """ Read-access of a symbol, like reading a variable. """
    Write = 3
    """ Write-access of a symbol, like writing to a variable. """


class CodeActionKind(Enum):
    """A set of predefined code action kinds"""

    Empty = ""
    """ Empty kind. """
    QuickFix = "quickfix"
    """ Base kind for quickfix actions: 'quickfix' """
    Refactor = "refactor"
    """ Base kind for refactoring actions: 'refactor' """
    RefactorExtract = "refactor.extract"
    """ Base kind for refactoring extraction actions: 'refactor.extract'

    Example extract actions:

    - Extract method
    - Extract function
    - Extract variable
    - Extract interface from class
    - ... """
    RefactorInline = "refactor.inline"
    """ Base kind for refactoring inline actions: 'refactor.inline'

    Example inline actions:

    - Inline function
    - Inline variable
    - Inline constant
    - ... """
    RefactorRewrite = "refactor.rewrite"
    """ Base kind for refactoring rewrite actions: 'refactor.rewrite'

    Example rewrite actions:

    - Convert JavaScript function to class
    - Add or remove parameter
    - Encapsulate field
    - Make method static
    - Move method to base class
    - ... """
    Source = "source"
    """ Base kind for source actions: `source`

    Source code actions apply to the entire file. """
    SourceOrganizeImports = "source.organizeImports"
    """ Base kind for an organize imports source action: `source.organizeImports` """
    SourceFixAll = "source.fixAll"
    """ Base kind for auto-fix source actions: `source.fixAll`.

    Fix all actions automatically fix errors that have a clear fix that do not require user input.
    They should not suppress errors or perform unsafe fixes such as generating new types or classes.

    @since 3.15.0 """


class TraceValues(Enum):
    Off = "off"
    """ Turn tracing off. """
    Messages = "messages"
    """ Trace messages only. """
    Verbose = "verbose"
    """ Verbose message tracing. """


class MarkupKind(Enum):
    """Describes the content type that a client supports in various
    result literals like `Hover`, `ParameterInfo` or `CompletionItem`.

    Please note that `MarkupKinds` must not start with a `$`. This kinds
    are reserved for internal usage."""

    PlainText = "plaintext"
    """ Plain text is supported as a content format """
    Markdown = "markdown"
    """ Markdown is supported as a content format """


class PositionEncodingKind(Enum):
    """A set of predefined position encoding kinds.

    @since 3.17.0"""

    UTF8 = "utf-8"
    """ Character offsets count UTF-8 code units. """
    UTF16 = "utf-16"
    """ Character offsets count UTF-16 code units.

    This is the default and must always be supported
    by servers """
    UTF32 = "utf-32"
    """ Character offsets count UTF-32 code units.

    Implementation note: these are the same as Unicode code points,
    so this `PositionEncodingKind` may also be used for an
    encoding-agnostic representation of character offsets. """


class FileChangeType(IntEnum):
    """The file event type"""

    Created = 1
    """ The file got created. """
    Changed = 2
    """ The file got changed. """
    Deleted = 3
    """ The file got deleted. """


class WatchKind(IntFlag):
    Create = 1
    """ Interested in create events. """
    Change = 2
    """ Interested in change events """
    Delete = 4
    """ Interested in delete events """


class DiagnosticSeverity(IntEnum):
    """The diagnostic's severity."""

    Error = 1
    """ Reports an error. """
    Warning = 2
    """ Reports a warning. """
    Information = 3
    """ Reports an information. """
    Hint = 4
    """ Reports a hint. """


class DiagnosticTag(IntEnum):
    """The diagnostic tags.

    @since 3.15.0"""

    Unnecessary = 1
    """ Unused or unnecessary code.

    Clients are allowed to render diagnostics with this tag faded out instead of having
    an error squiggle. """
    Deprecated = 2
    """ Deprecated or obsolete code.

    Clients are allowed to rendered diagnostics with this tag strike through. """


class CompletionTriggerKind(IntEnum):
    """How a completion was triggered"""

    Invoked = 1
    """ Completion was triggered by typing an identifier (24x7 code
    complete), manual invocation (e.g Ctrl+Space) or via API. """
    TriggerCharacter = 2
    """ Completion was triggered by a trigger character specified by
    the `triggerCharacters` properties of the `CompletionRegistrationOptions`. """
    TriggerForIncompleteCompletions = 3
    """ Completion was re-triggered as current completion list is incomplete """


class SignatureHelpTriggerKind(IntEnum):
    """How a signature help was triggered.

    @since 3.15.0"""

    Invoked = 1
    """ Signature help was invoked manually by the user or by a command. """
    TriggerCharacter = 2
    """ Signature help was triggered by a trigger character. """
    ContentChange = 3
    """ Signature help was triggered by the cursor moving or by the document content changing. """


class CodeActionTriggerKind(IntEnum):
    """The reason why code actions were requested.

    @since 3.17.0"""

    Invoked = 1
    """ Code actions were explicitly requested by the user or by an extension. """
    Automatic = 2
    """ Code actions were requested automatically.

    This typically happens when current selection in a file changes, but can
    also be triggered when file content changes. """


class FileOperationPatternKind(Enum):
    """A pattern kind describing if a glob pattern matches a file a folder or
    both.

    @since 3.16.0"""

    File = "file"
    """ The pattern matches a file only. """
    Folder = "folder"
    """ The pattern matches a folder only. """


class NotebookCellKind(IntEnum):
    """A notebook cell kind.

    @since 3.17.0"""

    Markup = 1
    """ A markup-cell is formatted source that is used for display. """
    Code = 2
    """ A code-cell is source code. """


class ResourceOperationKind(Enum):
    Create = "create"
    """ Supports creating new files and folders. """
    Rename = "rename"
    """ Supports renaming existing files and folders. """
    Delete = "delete"
    """ Supports deleting existing files and folders. """


class FailureHandlingKind(Enum):
    Abort = "abort"
    """ Applying the workspace change is simply aborted if one of the changes provided
    fails. All operations executed before the failing operation stay executed. """
    Transactional = "transactional"
    """ All operations are executed transactional. That means they either all
    succeed or no changes at all are applied to the workspace. """
    TextOnlyTransactional = "textOnlyTransactional"
    """ If the workspace edit contains only textual file changes they are executed transactional.
    If resource changes (create, rename or delete file) are part of the change the failure
    handling strategy is abort. """
    Undo = "undo"
    """ The client tries to undo the operations already executed. But there is no
    guarantee that this is succeeding. """


class PrepareSupportDefaultBehavior(IntEnum):
    Identifier = 1
    """ The client's default behavior is to select the identifier
    according the to language's syntax rule. """


class TokenFormat(Enum):
    Relative = "relative"


Definition = Union["Location", List["Location"]]
""" The definition of a symbol represented as one or many {@link Location locations}.
For most programming languages there is only one location at which a symbol is
defined.

Servers should prefer returning `DefinitionLink` over `Definition` if supported
by the client. """

DefinitionLink = "LocationLink"
""" Information about where a symbol is defined.

Provides additional metadata over normal {@link Location location} definitions, including the range of
the defining symbol """

LSPArray = List["LSPAny"]
""" LSP arrays.
@since 3.17.0 """

LSPAny = Union["LSPObject", "LSPArray", str, int, Uint, float, bool, None]
""" The LSP any type.
Please note that strictly speaking a property with the value `undefined`
can't be converted into JSON preserving the property name. However for
convenience it is allowed and assumed that all these properties are
optional as well.
@since 3.17.0 """

Declaration = Union["Location", List["Location"]]
""" The declaration of a symbol representation as one or many {@link Location locations}. """

DeclarationLink = "LocationLink"
""" Information about where a symbol is declared.

Provides additional metadata over normal {@link Location location} declarations, including the range of
the declaring symbol.

Servers should prefer returning `DeclarationLink` over `Declaration` if supported
by the client. """

InlineValue = Union["InlineValueText", "InlineValueVariableLookup", "InlineValueEvaluatableExpression"]
""" Inline value information can be provided by different means:
- directly as a text value (class InlineValueText).
- as a name to use for a variable lookup (class InlineValueVariableLookup)
- as an evaluatable expression (class InlineValueEvaluatableExpression)
The InlineValue types combines all inline value types into one type.

@since 3.17.0 """

DocumentDiagnosticReport = Union["RelatedFullDocumentDiagnosticReport", "RelatedUnchangedDocumentDiagnosticReport"]
""" The result of a document diagnostic pull request. A report can
either be a full report containing all diagnostics for the
requested document or an unchanged report indicating that nothing
has changed in terms of diagnostics in comparison to the last
pull request.

@since 3.17.0 """

PrepareRenameResult = Union["Range", "__PrepareRenameResult_Type_1", "__PrepareRenameResult_Type_2"]

DocumentSelector = List["DocumentFilter"]
""" A document selector is the combination of one or many document filters.

@sample `let sel:DocumentSelector = [{ language: 'typescript' }, { language: 'json', pattern: '**∕tsconfig.json' }]`;

The use of a string as a document filter is deprecated @since 3.16.0. """

ProgressToken = Union[int, str]

ChangeAnnotationIdentifier = str
""" An identifier to refer to a change annotation stored with a workspace edit. """

WorkspaceDocumentDiagnosticReport = Union[
    "WorkspaceFullDocumentDiagnosticReport",
    "WorkspaceUnchangedDocumentDiagnosticReport",
]
""" A workspace diagnostic document report.

@since 3.17.0 """

TextDocumentContentChangeEvent = Union[
    "__TextDocumentContentChangeEvent_Type_1", "__TextDocumentContentChangeEvent_Type_2"
]
""" An event describing a change to a text document. If only a text is provided
it is considered to be the full content of the document. """

MarkedString = Union[str, "__MarkedString_Type_1"]
""" MarkedString can be used to render human readable text. It is either a markdown string
or a code-block that provides a language and a code snippet. The language identifier
is semantically equal to the optional language identifier in fenced code blocks in GitHub
issues. See https://help.github.com/articles/creating-and-highlighting-code-blocks/#syntax-highlighting

The pair of a language and a value is an equivalent to markdown:
```${language}
${value}
```

Note that markdown strings will be sanitized - that means html will be escaped.
@deprecated use MarkupContent instead. """

DocumentFilter = Union["TextDocumentFilter", "NotebookCellTextDocumentFilter"]
""" A document filter describes a top level text document or
a notebook cell document.

@since 3.17.0 - proposed support for NotebookCellTextDocumentFilter. """

LSPObject = Dict[str, "LSPAny"]
""" LSP object definition.
@since 3.17.0 """

GlobPattern = Union["Pattern", "RelativePattern"]
""" The glob pattern. Either a string pattern or a relative pattern.

@since 3.17.0 """

TextDocumentFilter = Union[
    "__TextDocumentFilter_Type_1",
    "__TextDocumentFilter_Type_2",
    "__TextDocumentFilter_Type_3",
]
""" A document filter denotes a document by different properties like
the {@link TextDocument.languageId language}, the {@link Uri.scheme scheme} of
its resource, or a glob-pattern that is applied to the {@link TextDocument.fileName path}.

Glob patterns can have the following syntax:
- `*` to match one or more characters in a path segment
- `?` to match on one character in a path segment
- `**` to match any number of path segments, including none
- `{}` to group sub patterns into an OR expression. (e.g. `**​/*.{ts,js}` matches all TypeScript and JavaScript files)
- `[]` to declare a range of characters to match in a path segment (e.g., `example.[0-9]` to match on `example.0`, `example.1`, …)
- `[!...]` to negate a range of characters to match in a path segment (e.g., `example.[!0-9]` to match on `example.a`, `example.b`, but not `example.0`)

@sample A language filter that applies to typescript files on disk: `{ language: 'typescript', scheme: 'file' }`
@sample A language filter that applies to all package.json paths: `{ language: 'json', pattern: '**package.json' }`

@since 3.17.0 """

NotebookDocumentFilter = Union[
    "__NotebookDocumentFilter_Type_1",
    "__NotebookDocumentFilter_Type_2",
    "__NotebookDocumentFilter_Type_3",
]
""" A notebook document filter denotes a notebook document by
different properties. The properties will be match
against the notebook's URI (same as with documents)

@since 3.17.0 """

Pattern = str
""" The glob pattern to watch relative to the base path. Glob patterns can have the following syntax:
- `*` to match one or more characters in a path segment
- `?` to match on one character in a path segment
- `**` to match any number of path segments, including none
- `{}` to group conditions (e.g. `**​/*.{ts,js}` matches all TypeScript and JavaScript files)
- `[]` to declare a range of characters to match in a path segment (e.g., `example.[0-9]` to match on `example.0`, `example.1`, …)
- `[!...]` to negate a range of characters to match in a path segment (e.g., `example.[!0-9]` to match on `example.a`, `example.b`, but not `example.0`)

@since 3.17.0 """


class ImplementationParams(TypedDict):
    textDocument: "TextDocumentIdentifier"
    """ The text document. """
    position: "Position"
    """ The position inside the text document. """
    workDoneToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report work done progress. """
    partialResultToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report partial results (e.g. streaming) to
    the client. """


class Location(TypedDict):
    """Represents a location inside a resource, such as a line
    inside a text file."""

    uri: "DocumentUri"
    range: "Range"


class ImplementationRegistrationOptions(TypedDict):
    documentSelector: Union["DocumentSelector", None]
    """ A document selector to identify the scope of the registration. If set to null
    the document selector provided on the client side will be used. """
    id: NotRequired[str]
    """ The id used to register the request. The id can be used to deregister
    the request again. See also Registration#id. """


class TypeDefinitionParams(TypedDict):
    textDocument: "TextDocumentIdentifier"
    """ The text document. """
    position: "Position"
    """ The position inside the text document. """
    workDoneToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report work done progress. """
    partialResultToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report partial results (e.g. streaming) to
    the client. """


class TypeDefinitionRegistrationOptions(TypedDict):
    documentSelector: Union["DocumentSelector", None]
    """ A document selector to identify the scope of the registration. If set to null
    the document selector provided on the client side will be used. """
    id: NotRequired[str]
    """ The id used to register the request. The id can be used to deregister
    the request again. See also Registration#id. """


class WorkspaceFolder(TypedDict):
    """A workspace folder inside a client."""

    uri: "URI"
    """ The associated URI for this workspace folder. """
    name: str
    """ The name of the workspace folder. Used to refer to this
    workspace folder in the user interface. """


class DidChangeWorkspaceFoldersParams(TypedDict):
    """The parameters of a `workspace/didChangeWorkspaceFolders` notification."""

    event: "WorkspaceFoldersChangeEvent"
    """ The actual workspace folder change event. """


class ConfigurationParams(TypedDict):
    """The parameters of a configuration request."""

    items: List["ConfigurationItem"]


class DocumentColorParams(TypedDict):
    """Parameters for a {@link DocumentColorRequest}."""

    textDocument: "TextDocumentIdentifier"
    """ The text document. """
    workDoneToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report work done progress. """
    partialResultToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report partial results (e.g. streaming) to
    the client. """


class ColorInformation(TypedDict):
    """Represents a color range from a document."""

    range: "Range"
    """ The range in the document where this color appears. """
    color: "Color"
    """ The actual color value for this color range. """


class DocumentColorRegistrationOptions(TypedDict):
    documentSelector: Union["DocumentSelector", None]
    """ A document selector to identify the scope of the registration. If set to null
    the document selector provided on the client side will be used. """
    id: NotRequired[str]
    """ The id used to register the request. The id can be used to deregister
    the request again. See also Registration#id. """


class ColorPresentationParams(TypedDict):
    """Parameters for a {@link ColorPresentationRequest}."""

    textDocument: "TextDocumentIdentifier"
    """ The text document. """
    color: "Color"
    """ The color to request presentations for. """
    range: "Range"
    """ The range where the color would be inserted. Serves as a context. """
    workDoneToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report work done progress. """
    partialResultToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report partial results (e.g. streaming) to
    the client. """


class ColorPresentation(TypedDict):
    label: str
    """ The label of this color presentation. It will be shown on the color
    picker header. By default this is also the text that is inserted when selecting
    this color presentation. """
    textEdit: NotRequired["TextEdit"]
    """ An {@link TextEdit edit} which is applied to a document when selecting
    this presentation for the color.  When `falsy` the {@link ColorPresentation.label label}
    is used. """
    additionalTextEdits: NotRequired[List["TextEdit"]]
    """ An optional array of additional {@link TextEdit text edits} that are applied when
    selecting this color presentation. Edits must not overlap with the main {@link ColorPresentation.textEdit edit} nor with themselves. """


class WorkDoneProgressOptions(TypedDict):
    workDoneProgress: NotRequired[bool]


class TextDocumentRegistrationOptions(TypedDict):
    """General text document registration options."""

    documentSelector: Union["DocumentSelector", None]
    """ A document selector to identify the scope of the registration. If set to null
    the document selector provided on the client side will be used. """


class FoldingRangeParams(TypedDict):
    """Parameters for a {@link FoldingRangeRequest}."""

    textDocument: "TextDocumentIdentifier"
    """ The text document. """
    workDoneToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report work done progress. """
    partialResultToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report partial results (e.g. streaming) to
    the client. """


class FoldingRange(TypedDict):
    """Represents a folding range. To be valid, start and end line must be bigger than zero and smaller
    than the number of lines in the document. Clients are free to ignore invalid ranges.
    """

    startLine: Uint
    """ The zero-based start line of the range to fold. The folded area starts after the line's last character.
    To be valid, the end must be zero or larger and smaller than the number of lines in the document. """
    startCharacter: NotRequired[Uint]
    """ The zero-based character offset from where the folded range starts. If not defined, defaults to the length of the start line. """
    endLine: Uint
    """ The zero-based end line of the range to fold. The folded area ends with the line's last character.
    To be valid, the end must be zero or larger and smaller than the number of lines in the document. """
    endCharacter: NotRequired[Uint]
    """ The zero-based character offset before the folded range ends. If not defined, defaults to the length of the end line. """
    kind: NotRequired["FoldingRangeKind"]
    """ Describes the kind of the folding range such as `comment' or 'region'. The kind
    is used to categorize folding ranges and used by commands like 'Fold all comments'.
    See {@link FoldingRangeKind} for an enumeration of standardized kinds. """
    collapsedText: NotRequired[str]
    """ The text that the client should show when the specified range is
    collapsed. If not defined or not supported by the client, a default
    will be chosen by the client.

    @since 3.17.0 """


class FoldingRangeRegistrationOptions(TypedDict):
    documentSelector: Union["DocumentSelector", None]
    """ A document selector to identify the scope of the registration. If set to null
    the document selector provided on the client side will be used. """
    id: NotRequired[str]
    """ The id used to register the request. The id can be used to deregister
    the request again. See also Registration#id. """


class DeclarationParams(TypedDict):
    textDocument: "TextDocumentIdentifier"
    """ The text document. """
    position: "Position"
    """ The position inside the text document. """
    workDoneToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report work done progress. """
    partialResultToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report partial results (e.g. streaming) to
    the client. """


class DeclarationRegistrationOptions(TypedDict):
    documentSelector: Union["DocumentSelector", None]
    """ A document selector to identify the scope of the registration. If set to null
    the document selector provided on the client side will be used. """
    id: NotRequired[str]
    """ The id used to register the request. The id can be used to deregister
    the request again. See also Registration#id. """


class SelectionRangeParams(TypedDict):
    """A parameter literal used in selection range requests."""

    textDocument: "TextDocumentIdentifier"
    """ The text document. """
    positions: List["Position"]
    """ The positions inside the text document. """
    workDoneToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report work done progress. """
    partialResultToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report partial results (e.g. streaming) to
    the client. """


class SelectionRange(TypedDict):
    """A selection range represents a part of a selection hierarchy. A selection range
    may have a parent selection range that contains it."""

    range: "Range"
    """ The {@link Range range} of this selection range. """
    parent: NotRequired["SelectionRange"]
    """ The parent selection range containing this range. Therefore `parent.range` must contain `this.range`. """


class SelectionRangeRegistrationOptions(TypedDict):
    documentSelector: Union["DocumentSelector", None]
    """ A document selector to identify the scope of the registration. If set to null
    the document selector provided on the client side will be used. """
    id: NotRequired[str]
    """ The id used to register the request. The id can be used to deregister
    the request again. See also Registration#id. """


class WorkDoneProgressCreateParams(TypedDict):
    token: "ProgressToken"
    """ The token to be used to report progress. """


class WorkDoneProgressCancelParams(TypedDict):
    token: "ProgressToken"
    """ The token to be used to report progress. """


class CallHierarchyPrepareParams(TypedDict):
    """The parameter of a `textDocument/prepareCallHierarchy` request.

    @since 3.16.0"""

    textDocument: "TextDocumentIdentifier"
    """ The text document. """
    position: "Position"
    """ The position inside the text document. """
    workDoneToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report work done progress. """


class CallHierarchyItem(TypedDict):
    """Represents programming constructs like functions or constructors in the context
    of call hierarchy.

    @since 3.16.0"""

    name: str
    """ The name of this item. """
    kind: "SymbolKind"
    """ The kind of this item. """
    tags: NotRequired[List["SymbolTag"]]
    """ Tags for this item. """
    detail: NotRequired[str]
    """ More detail for this item, e.g. the signature of a function. """
    uri: "DocumentUri"
    """ The resource identifier of this item. """
    range: "Range"
    """ The range enclosing this symbol not including leading/trailing whitespace but everything else, e.g. comments and code. """
    selectionRange: "Range"
    """ The range that should be selected and revealed when this symbol is being picked, e.g. the name of a function.
    Must be contained by the {@link CallHierarchyItem.range `range`}. """
    data: NotRequired["LSPAny"]
    """ A data entry field that is preserved between a call hierarchy prepare and
    incoming calls or outgoing calls requests. """


class CallHierarchyRegistrationOptions(TypedDict):
    """Call hierarchy options used during static or dynamic registration.

    @since 3.16.0"""

    documentSelector: Union["DocumentSelector", None]
    """ A document selector to identify the scope of the registration. If set to null
    the document selector provided on the client side will be used. """
    id: NotRequired[str]
    """ The id used to register the request. The id can be used to deregister
    the request again. See also Registration#id. """


class CallHierarchyIncomingCallsParams(TypedDict):
    """The parameter of a `callHierarchy/incomingCalls` request.

    @since 3.16.0"""

    item: "CallHierarchyItem"
    workDoneToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report work done progress. """
    partialResultToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report partial results (e.g. streaming) to
    the client. """


CallHierarchyIncomingCall = TypedDict(
    "CallHierarchyIncomingCall",
    {
        # The item that makes the call.
        "from": "CallHierarchyItem",
        # The ranges at which the calls appear. This is relative to the caller
        # denoted by {@link CallHierarchyIncomingCall.from `this.from`}.
        "fromRanges": List["Range"],
    },
)
""" Represents an incoming call, e.g. a caller of a method or constructor.

@since 3.16.0 """


class CallHierarchyOutgoingCallsParams(TypedDict):
    """The parameter of a `callHierarchy/outgoingCalls` request.

    @since 3.16.0"""

    item: "CallHierarchyItem"
    workDoneToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report work done progress. """
    partialResultToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report partial results (e.g. streaming) to
    the client. """


class CallHierarchyOutgoingCall(TypedDict):
    """Represents an outgoing call, e.g. calling a getter from a method or a method from a constructor etc.

    @since 3.16.0"""

    to: "CallHierarchyItem"
    """ The item that is called. """
    fromRanges: List["Range"]
    """ The range at which this item is called. This is the range relative to the caller, e.g the item
    passed to {@link CallHierarchyItemProvider.provideCallHierarchyOutgoingCalls `provideCallHierarchyOutgoingCalls`}
    and not {@link CallHierarchyOutgoingCall.to `this.to`}. """


class SemanticTokensParams(TypedDict):
    """@since 3.16.0"""

    textDocument: "TextDocumentIdentifier"
    """ The text document. """
    workDoneToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report work done progress. """
    partialResultToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report partial results (e.g. streaming) to
    the client. """


class SemanticTokens(TypedDict):
    """@since 3.16.0"""

    resultId: NotRequired[str]
    """ An optional result id. If provided and clients support delta updating
    the client will include the result id in the next semantic token request.
    A server can then instead of computing all semantic tokens again simply
    send a delta. """
    data: List[Uint]
    """ The actual tokens. """


class SemanticTokensPartialResult(TypedDict):
    """@since 3.16.0"""

    data: List[Uint]


class SemanticTokensRegistrationOptions(TypedDict):
    """@since 3.16.0"""

    documentSelector: Union["DocumentSelector", None]
    """ A document selector to identify the scope of the registration. If set to null
    the document selector provided on the client side will be used. """
    legend: "SemanticTokensLegend"
    """ The legend used by the server """
    range: NotRequired[Union[bool, dict]]
    """ Server supports providing semantic tokens for a specific range
    of a document. """
    full: NotRequired[Union[bool, "__SemanticTokensOptions_full_Type_1"]]
    """ Server supports providing semantic tokens for a full document. """
    id: NotRequired[str]
    """ The id used to register the request. The id can be used to deregister
    the request again. See also Registration#id. """


class SemanticTokensDeltaParams(TypedDict):
    """@since 3.16.0"""

    textDocument: "TextDocumentIdentifier"
    """ The text document. """
    previousResultId: str
    """ The result id of a previous response. The result Id can either point to a full response
    or a delta response depending on what was received last. """
    workDoneToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report work done progress. """
    partialResultToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report partial results (e.g. streaming) to
    the client. """


class SemanticTokensDelta(TypedDict):
    """@since 3.16.0"""

    resultId: NotRequired[str]
    edits: List["SemanticTokensEdit"]
    """ The semantic token edits to transform a previous result into a new result. """


class SemanticTokensDeltaPartialResult(TypedDict):
    """@since 3.16.0"""

    edits: List["SemanticTokensEdit"]


class SemanticTokensRangeParams(TypedDict):
    """@since 3.16.0"""

    textDocument: "TextDocumentIdentifier"
    """ The text document. """
    range: "Range"
    """ The range the semantic tokens are requested for. """
    workDoneToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report work done progress. """
    partialResultToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report partial results (e.g. streaming) to
    the client. """


class ShowDocumentParams(TypedDict):
    """Params to show a document.

    @since 3.16.0"""

    uri: "URI"
    """ The document uri to show. """
    external: NotRequired[bool]
    """ Indicates to show the resource in an external program.
    To show for example `https://code.visualstudio.com/`
    in the default WEB browser set `external` to `true`. """
    takeFocus: NotRequired[bool]
    """ An optional property to indicate whether the editor
    showing the document should take focus or not.
    Clients might ignore this property if an external
    program is started. """
    selection: NotRequired["Range"]
    """ An optional selection range if the document is a text
    document. Clients might ignore the property if an
    external program is started or the file is not a text
    file. """


class ShowDocumentResult(TypedDict):
    """The result of a showDocument request.

    @since 3.16.0"""

    success: bool
    """ A boolean indicating if the show was successful. """


class LinkedEditingRangeParams(TypedDict):
    textDocument: "TextDocumentIdentifier"
    """ The text document. """
    position: "Position"
    """ The position inside the text document. """
    workDoneToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report work done progress. """


class LinkedEditingRanges(TypedDict):
    """The result of a linked editing range request.

    @since 3.16.0"""

    ranges: List["Range"]
    """ A list of ranges that can be edited together. The ranges must have
    identical length and contain identical text content. The ranges cannot overlap. """
    wordPattern: NotRequired[str]
    """ An optional word pattern (regular expression) that describes valid contents for
    the given ranges. If no pattern is provided, the client configuration's word
    pattern will be used. """


class LinkedEditingRangeRegistrationOptions(TypedDict):
    documentSelector: Union["DocumentSelector", None]
    """ A document selector to identify the scope of the registration. If set to null
    the document selector provided on the client side will be used. """
    id: NotRequired[str]
    """ The id used to register the request. The id can be used to deregister
    the request again. See also Registration#id. """


class CreateFilesParams(TypedDict):
    """The parameters sent in notifications/requests for user-initiated creation of
    files.

    @since 3.16.0"""

    files: List["FileCreate"]
    """ An array of all files/folders created in this operation. """


class WorkspaceEdit(TypedDict):
    """A workspace edit represents changes to many resources managed in the workspace. The edit
    should either provide `changes` or `documentChanges`. If documentChanges are present
    they are preferred over `changes` if the client can handle versioned document edits.

    Since version 3.13.0 a workspace edit can contain resource operations as well. If resource
    operations are present clients need to execute the operations in the order in which they
    are provided. So a workspace edit for example can consist of the following two changes:
    (1) a create file a.txt and (2) a text document edit which insert text into file a.txt.

    An invalid sequence (e.g. (1) delete file a.txt and (2) insert text into file a.txt) will
    cause failure of the operation. How the client recovers from the failure is described by
    the client capability: `workspace.workspaceEdit.failureHandling`"""

    changes: NotRequired[Dict["DocumentUri", List["TextEdit"]]]
    """ Holds changes to existing resources. """
    documentChanges: NotRequired[List[Union["TextDocumentEdit", "CreateFile", "RenameFile", "DeleteFile"]]]
    """ Depending on the client capability `workspace.workspaceEdit.resourceOperations` document changes
    are either an array of `TextDocumentEdit`s to express changes to n different text documents
    where each text document edit addresses a specific version of a text document. Or it can contain
    above `TextDocumentEdit`s mixed with create, rename and delete file / folder operations.

    Whether a client supports versioned document edits is expressed via
    `workspace.workspaceEdit.documentChanges` client capability.

    If a client neither supports `documentChanges` nor `workspace.workspaceEdit.resourceOperations` then
    only plain `TextEdit`s using the `changes` property are supported. """
    changeAnnotations: NotRequired[Dict["ChangeAnnotationIdentifier", "ChangeAnnotation"]]
    """ A map of change annotations that can be referenced in `AnnotatedTextEdit`s or create, rename and
    delete file / folder operations.

    Whether clients honor this property depends on the client capability `workspace.changeAnnotationSupport`.

    @since 3.16.0 """


class FileOperationRegistrationOptions(TypedDict):
    """The options to register for file operations.

    @since 3.16.0"""

    filters: List["FileOperationFilter"]
    """ The actual filters. """


class RenameFilesParams(TypedDict):
    """The parameters sent in notifications/requests for user-initiated renames of
    files.

    @since 3.16.0"""

    files: List["FileRename"]
    """ An array of all files/folders renamed in this operation. When a folder is renamed, only
    the folder will be included, and not its children. """


class DeleteFilesParams(TypedDict):
    """The parameters sent in notifications/requests for user-initiated deletes of
    files.

    @since 3.16.0"""

    files: List["FileDelete"]
    """ An array of all files/folders deleted in this operation. """


class MonikerParams(TypedDict):
    textDocument: "TextDocumentIdentifier"
    """ The text document. """
    position: "Position"
    """ The position inside the text document. """
    workDoneToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report work done progress. """
    partialResultToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report partial results (e.g. streaming) to
    the client. """


class Moniker(TypedDict):
    """Moniker definition to match LSIF 0.5 moniker definition.

    @since 3.16.0"""

    scheme: str
    """ The scheme of the moniker. For example tsc or .Net """
    identifier: str
    """ The identifier of the moniker. The value is opaque in LSIF however
    schema owners are allowed to define the structure if they want. """
    unique: "UniquenessLevel"
    """ The scope in which the moniker is unique """
    kind: NotRequired["MonikerKind"]
    """ The moniker kind if known. """


class MonikerRegistrationOptions(TypedDict):
    documentSelector: Union["DocumentSelector", None]
    """ A document selector to identify the scope of the registration. If set to null
    the document selector provided on the client side will be used. """


class TypeHierarchyPrepareParams(TypedDict):
    """The parameter of a `textDocument/prepareTypeHierarchy` request.

    @since 3.17.0"""

    textDocument: "TextDocumentIdentifier"
    """ The text document. """
    position: "Position"
    """ The position inside the text document. """
    workDoneToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report work done progress. """


class TypeHierarchyItem(TypedDict):
    """@since 3.17.0"""

    name: str
    """ The name of this item. """
    kind: "SymbolKind"
    """ The kind of this item. """
    tags: NotRequired[List["SymbolTag"]]
    """ Tags for this item. """
    detail: NotRequired[str]
    """ More detail for this item, e.g. the signature of a function. """
    uri: "DocumentUri"
    """ The resource identifier of this item. """
    range: "Range"
    """ The range enclosing this symbol not including leading/trailing whitespace
    but everything else, e.g. comments and code. """
    selectionRange: "Range"
    """ The range that should be selected and revealed when this symbol is being
    picked, e.g. the name of a function. Must be contained by the
    {@link TypeHierarchyItem.range `range`}. """
    data: NotRequired["LSPAny"]
    """ A data entry field that is preserved between a type hierarchy prepare and
    supertypes or subtypes requests. It could also be used to identify the
    type hierarchy in the server, helping improve the performance on
    resolving supertypes and subtypes. """


class TypeHierarchyRegistrationOptions(TypedDict):
    """Type hierarchy options used during static or dynamic registration.

    @since 3.17.0"""

    documentSelector: Union["DocumentSelector", None]
    """ A document selector to identify the scope of the registration. If set to null
    the document selector provided on the client side will be used. """
    id: NotRequired[str]
    """ The id used to register the request. The id can be used to deregister
    the request again. See also Registration#id. """


class TypeHierarchySupertypesParams(TypedDict):
    """The parameter of a `typeHierarchy/supertypes` request.

    @since 3.17.0"""

    item: "TypeHierarchyItem"
    workDoneToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report work done progress. """
    partialResultToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report partial results (e.g. streaming) to
    the client. """


class TypeHierarchySubtypesParams(TypedDict):
    """The parameter of a `typeHierarchy/subtypes` request.

    @since 3.17.0"""

    item: "TypeHierarchyItem"
    workDoneToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report work done progress. """
    partialResultToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report partial results (e.g. streaming) to
    the client. """


class InlineValueParams(TypedDict):
    """A parameter literal used in inline value requests.

    @since 3.17.0"""

    textDocument: "TextDocumentIdentifier"
    """ The text document. """
    range: "Range"
    """ The document range for which inline values should be computed. """
    context: "InlineValueContext"
    """ Additional information about the context in which inline values were
    requested. """
    workDoneToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report work done progress. """


class InlineValueRegistrationOptions(TypedDict):
    """Inline value options used during static or dynamic registration.

    @since 3.17.0"""

    documentSelector: Union["DocumentSelector", None]
    """ A document selector to identify the scope of the registration. If set to null
    the document selector provided on the client side will be used. """
    id: NotRequired[str]
    """ The id used to register the request. The id can be used to deregister
    the request again. See also Registration#id. """


class InlayHintParams(TypedDict):
    """A parameter literal used in inlay hint requests.

    @since 3.17.0"""

    textDocument: "TextDocumentIdentifier"
    """ The text document. """
    range: "Range"
    """ The document range for which inlay hints should be computed. """
    workDoneToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report work done progress. """


class InlayHint(TypedDict):
    """Inlay hint information.

    @since 3.17.0"""

    position: "Position"
    """ The position of this hint. """
    label: Union[str, List["InlayHintLabelPart"]]
    """ The label of this hint. A human readable string or an array of
    InlayHintLabelPart label parts.

    *Note* that neither the string nor the label part can be empty. """
    kind: NotRequired["InlayHintKind"]
    """ The kind of this hint. Can be omitted in which case the client
    should fall back to a reasonable default. """
    textEdits: NotRequired[List["TextEdit"]]
    """ Optional text edits that are performed when accepting this inlay hint.

    *Note* that edits are expected to change the document so that the inlay
    hint (or its nearest variant) is now part of the document and the inlay
    hint itself is now obsolete. """
    tooltip: NotRequired[Union[str, "MarkupContent"]]
    """ The tooltip text when you hover over this item. """
    paddingLeft: NotRequired[bool]
    """ Render padding before the hint.

    Note: Padding should use the editor's background color, not the
    background color of the hint itself. That means padding can be used
    to visually align/separate an inlay hint. """
    paddingRight: NotRequired[bool]
    """ Render padding after the hint.

    Note: Padding should use the editor's background color, not the
    background color of the hint itself. That means padding can be used
    to visually align/separate an inlay hint. """
    data: NotRequired["LSPAny"]
    """ A data entry field that is preserved on an inlay hint between
    a `textDocument/inlayHint` and a `inlayHint/resolve` request. """


class InlayHintRegistrationOptions(TypedDict):
    """Inlay hint options used during static or dynamic registration.

    @since 3.17.0"""

    resolveProvider: NotRequired[bool]
    """ The server provides support to resolve additional
    information for an inlay hint item. """
    documentSelector: Union["DocumentSelector", None]
    """ A document selector to identify the scope of the registration. If set to null
    the document selector provided on the client side will be used. """
    id: NotRequired[str]
    """ The id used to register the request. The id can be used to deregister
    the request again. See also Registration#id. """


class DocumentDiagnosticParams(TypedDict):
    """Parameters of the document diagnostic request.

    @since 3.17.0"""

    textDocument: "TextDocumentIdentifier"
    """ The text document. """
    identifier: NotRequired[str]
    """ The additional identifier  provided during registration. """
    previousResultId: NotRequired[str]
    """ The result id of a previous response if provided. """
    workDoneToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report work done progress. """
    partialResultToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report partial results (e.g. streaming) to
    the client. """


class DocumentDiagnosticReportPartialResult(TypedDict):
    """A partial result for a document diagnostic report.

    @since 3.17.0"""

    relatedDocuments: Dict[
        "DocumentUri",
        Union["FullDocumentDiagnosticReport", "UnchangedDocumentDiagnosticReport"],
    ]


class DiagnosticServerCancellationData(TypedDict):
    """Cancellation data returned from a diagnostic request.

    @since 3.17.0"""

    retriggerRequest: bool


class DiagnosticRegistrationOptions(TypedDict):
    """Diagnostic registration options.

    @since 3.17.0"""

    documentSelector: Union["DocumentSelector", None]
    """ A document selector to identify the scope of the registration. If set to null
    the document selector provided on the client side will be used. """
    identifier: NotRequired[str]
    """ An optional identifier under which the diagnostics are
    managed by the client. """
    interFileDependencies: bool
    """ Whether the language has inter file dependencies meaning that
    editing code in one file can result in a different diagnostic
    set in another file. Inter file dependencies are common for
    most programming languages and typically uncommon for linters. """
    workspaceDiagnostics: bool
    """ The server provides support for workspace diagnostics as well. """
    id: NotRequired[str]
    """ The id used to register the request. The id can be used to deregister
    the request again. See also Registration#id. """


class WorkspaceDiagnosticParams(TypedDict):
    """Parameters of the workspace diagnostic request.

    @since 3.17.0"""

    identifier: NotRequired[str]
    """ The additional identifier provided during registration. """
    previousResultIds: List["PreviousResultId"]
    """ The currently known diagnostic reports with their
    previous result ids. """
    workDoneToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report work done progress. """
    partialResultToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report partial results (e.g. streaming) to
    the client. """


class WorkspaceDiagnosticReport(TypedDict):
    """A workspace diagnostic report.

    @since 3.17.0"""

    items: List["WorkspaceDocumentDiagnosticReport"]


class WorkspaceDiagnosticReportPartialResult(TypedDict):
    """A partial result for a workspace diagnostic report.

    @since 3.17.0"""

    items: List["WorkspaceDocumentDiagnosticReport"]


class DidOpenNotebookDocumentParams(TypedDict):
    """The params sent in an open notebook document notification.

    @since 3.17.0"""

    notebookDocument: "NotebookDocument"
    """ The notebook document that got opened. """
    cellTextDocuments: List["TextDocumentItem"]
    """ The text documents that represent the content
    of a notebook cell. """


class DidChangeNotebookDocumentParams(TypedDict):
    """The params sent in a change notebook document notification.

    @since 3.17.0"""

    notebookDocument: "VersionedNotebookDocumentIdentifier"
    """ The notebook document that did change. The version number points
    to the version after all provided changes have been applied. If
    only the text document content of a cell changes the notebook version
    doesn't necessarily have to change. """
    change: "NotebookDocumentChangeEvent"
    """ The actual changes to the notebook document.

    The changes describe single state changes to the notebook document.
    So if there are two changes c1 (at array index 0) and c2 (at array
    index 1) for a notebook in state S then c1 moves the notebook from
    S to S' and c2 from S' to S''. So c1 is computed on the state S and
    c2 is computed on the state S'.

    To mirror the content of a notebook using change events use the following approach:
    - start with the same initial content
    - apply the 'notebookDocument/didChange' notifications in the order you receive them.
    - apply the `NotebookChangeEvent`s in a single notification in the order
      you receive them. """


class DidSaveNotebookDocumentParams(TypedDict):
    """The params sent in a save notebook document notification.

    @since 3.17.0"""

    notebookDocument: "NotebookDocumentIdentifier"
    """ The notebook document that got saved. """


class DidCloseNotebookDocumentParams(TypedDict):
    """The params sent in a close notebook document notification.

    @since 3.17.0"""

    notebookDocument: "NotebookDocumentIdentifier"
    """ The notebook document that got closed. """
    cellTextDocuments: List["TextDocumentIdentifier"]
    """ The text documents that represent the content
    of a notebook cell that got closed. """


class RegistrationParams(TypedDict):
    registrations: List["Registration"]


class UnregistrationParams(TypedDict):
    unregisterations: List["Unregistration"]


class InitializeParams(TypedDict):
    processId: Union[int, None]
    """ The process Id of the parent process that started
    the server.

    Is `null` if the process has not been started by another process.
    If the parent process is not alive then the server should exit. """
    clientInfo: NotRequired["___InitializeParams_clientInfo_Type_1"]
    """ Information about the client

    @since 3.15.0 """
    locale: NotRequired[str]
    """ The locale the client is currently showing the user interface
    in. This must not necessarily be the locale of the operating
    system.

    Uses IETF language tags as the value's syntax
    (See https://en.wikipedia.org/wiki/IETF_language_tag)

    @since 3.16.0 """
    rootPath: NotRequired[Union[str, None]]
    """ The rootPath of the workspace. Is null
    if no folder is open.

    @deprecated in favour of rootUri. """
    rootUri: Union["DocumentUri", None]
    """ The rootUri of the workspace. Is null if no
    folder is open. If both `rootPath` and `rootUri` are set
    `rootUri` wins.

    @deprecated in favour of workspaceFolders. """
    capabilities: "ClientCapabilities"
    """ The capabilities provided by the client (editor or tool) """
    initializationOptions: NotRequired["LSPAny"]
    """ User provided initialization options. """
    trace: NotRequired["TraceValues"]
    """ The initial trace setting. If omitted trace is disabled ('off'). """
    workspaceFolders: NotRequired[Union[List["WorkspaceFolder"], None]]
    """ The workspace folders configured in the client when the server starts.

    This property is only available if the client supports workspace folders.
    It can be `null` if the client supports workspace folders but none are
    configured.

    @since 3.6.0 """


class InitializeResult(TypedDict):
    """The result returned from an initialize request."""

    capabilities: "ServerCapabilities"
    """ The capabilities the language server provides. """
    serverInfo: NotRequired["__InitializeResult_serverInfo_Type_1"]
    """ Information about the server.

    @since 3.15.0 """


class InitializeError(TypedDict):
    """The data type of the ResponseError if the
    initialize request fails."""

    retry: bool
    """ Indicates whether the client execute the following retry logic:
    (1) show the message provided by the ResponseError to the user
    (2) user selects retry or cancel
    (3) if user selected retry the initialize method is sent again. """


class InitializedParams(TypedDict):
    pass


class DidChangeConfigurationParams(TypedDict):
    """The parameters of a change configuration notification."""

    settings: "LSPAny"
    """ The actual changed settings """


class DidChangeConfigurationRegistrationOptions(TypedDict):
    section: NotRequired[Union[str, List[str]]]


class ShowMessageParams(TypedDict):
    """The parameters of a notification message."""

    type: "MessageType"
    """ The message type. See {@link MessageType} """
    message: str
    """ The actual message. """


class ShowMessageRequestParams(TypedDict):
    type: "MessageType"
    """ The message type. See {@link MessageType} """
    message: str
    """ The actual message. """
    actions: NotRequired[List["MessageActionItem"]]
    """ The message action items to present. """


class MessageActionItem(TypedDict):
    title: str
    """ A short title like 'Retry', 'Open Log' etc. """


class LogMessageParams(TypedDict):
    """The log message parameters."""

    type: "MessageType"
    """ The message type. See {@link MessageType} """
    message: str
    """ The actual message. """


class DidOpenTextDocumentParams(TypedDict):
    """The parameters sent in an open text document notification"""

    textDocument: "TextDocumentItem"
    """ The document that was opened. """


class DidChangeTextDocumentParams(TypedDict):
    """The change text document notification's parameters."""

    textDocument: "VersionedTextDocumentIdentifier"
    """ The document that did change. The version number points
    to the version after all provided content changes have
    been applied. """
    contentChanges: List["TextDocumentContentChangeEvent"]
    """ The actual content changes. The content changes describe single state changes
    to the document. So if there are two content changes c1 (at array index 0) and
    c2 (at array index 1) for a document in state S then c1 moves the document from
    S to S' and c2 from S' to S''. So c1 is computed on the state S and c2 is computed
    on the state S'.

    To mirror the content of a document using change events use the following approach:
    - start with the same initial content
    - apply the 'textDocument/didChange' notifications in the order you receive them.
    - apply the `TextDocumentContentChangeEvent`s in a single notification in the order
      you receive them. """


class TextDocumentChangeRegistrationOptions(TypedDict):
    """Describe options to be used when registered for text document change events."""

    syncKind: "TextDocumentSyncKind"
    """ How documents are synced to the server. """
    documentSelector: Union["DocumentSelector", None]
    """ A document selector to identify the scope of the registration. If set to null
    the document selector provided on the client side will be used. """


class DidCloseTextDocumentParams(TypedDict):
    """The parameters sent in a close text document notification"""

    textDocument: "TextDocumentIdentifier"
    """ The document that was closed. """


class DidSaveTextDocumentParams(TypedDict):
    """The parameters sent in a save text document notification"""

    textDocument: "TextDocumentIdentifier"
    """ The document that was saved. """
    text: NotRequired[str]
    """ Optional the content when saved. Depends on the includeText value
    when the save notification was requested. """


class TextDocumentSaveRegistrationOptions(TypedDict):
    """Save registration options."""

    documentSelector: Union["DocumentSelector", None]
    """ A document selector to identify the scope of the registration. If set to null
    the document selector provided on the client side will be used. """
    includeText: NotRequired[bool]
    """ The client is supposed to include the content on save. """


class WillSaveTextDocumentParams(TypedDict):
    """The parameters sent in a will save text document notification."""

    textDocument: "TextDocumentIdentifier"
    """ The document that will be saved. """
    reason: "TextDocumentSaveReason"
    """ The 'TextDocumentSaveReason'. """


class TextEdit(TypedDict):
    """A text edit applicable to a text document."""

    range: "Range"
    """ The range of the text document to be manipulated. To insert
    text into a document create a range where start === end. """
    newText: str
    """ The string to be inserted. For delete operations use an
    empty string. """


class DidChangeWatchedFilesParams(TypedDict):
    """The watched files change notification's parameters."""

    changes: List["FileEvent"]
    """ The actual file events. """


class DidChangeWatchedFilesRegistrationOptions(TypedDict):
    """Describe options to be used when registered for text document change events."""

    watchers: List["FileSystemWatcher"]
    """ The watchers to register. """


class PublishDiagnosticsParams(TypedDict):
    """The publish diagnostic notification's parameters."""

    uri: "DocumentUri"
    """ The URI for which diagnostic information is reported. """
    version: NotRequired[int]
    """ Optional the version number of the document the diagnostics are published for.

    @since 3.15.0 """
    diagnostics: List["Diagnostic"]
    """ An array of diagnostic information items. """


class CompletionParams(TypedDict):
    """Completion parameters"""

    context: NotRequired["CompletionContext"]
    """ The completion context. This is only available it the client specifies
    to send this using the client capability `textDocument.completion.contextSupport === true` """
    textDocument: "TextDocumentIdentifier"
    """ The text document. """
    position: "Position"
    """ The position inside the text document. """
    workDoneToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report work done progress. """
    partialResultToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report partial results (e.g. streaming) to
    the client. """


class CompletionItem(TypedDict):
    """A completion item represents a text snippet that is
    proposed to complete text that is being typed."""

    label: str
    """ The label of this completion item.

    The label property is also by default the text that
    is inserted when selecting this completion.

    If label details are provided the label itself should
    be an unqualified name of the completion item. """
    labelDetails: NotRequired["CompletionItemLabelDetails"]
    """ Additional details for the label

    @since 3.17.0 """
    kind: NotRequired["CompletionItemKind"]
    """ The kind of this completion item. Based of the kind
    an icon is chosen by the editor. """
    tags: NotRequired[List["CompletionItemTag"]]
    """ Tags for this completion item.

    @since 3.15.0 """
    detail: NotRequired[str]
    """ A human-readable string with additional information
    about this item, like type or symbol information. """
    documentation: NotRequired[Union[str, "MarkupContent"]]
    """ A human-readable string that represents a doc-comment. """
    deprecated: NotRequired[bool]
    """ Indicates if this item is deprecated.
    @deprecated Use `tags` instead. """
    preselect: NotRequired[bool]
    """ Select this item when showing.

    *Note* that only one completion item can be selected and that the
    tool / client decides which item that is. The rule is that the *first*
    item of those that match best is selected. """
    sortText: NotRequired[str]
    """ A string that should be used when comparing this item
    with other items. When `falsy` the {@link CompletionItem.label label}
    is used. """
    filterText: NotRequired[str]
    """ A string that should be used when filtering a set of
    completion items. When `falsy` the {@link CompletionItem.label label}
    is used. """
    insertText: NotRequired[str]
    """ A string that should be inserted into a document when selecting
    this completion. When `falsy` the {@link CompletionItem.label label}
    is used.

    The `insertText` is subject to interpretation by the client side.
    Some tools might not take the string literally. For example
    VS Code when code complete is requested in this example
    `con<cursor position>` and a completion item with an `insertText` of
    `console` is provided it will only insert `sole`. Therefore it is
    recommended to use `textEdit` instead since it avoids additional client
    side interpretation. """
    insertTextFormat: NotRequired["InsertTextFormat"]
    """ The format of the insert text. The format applies to both the
    `insertText` property and the `newText` property of a provided
    `textEdit`. If omitted defaults to `InsertTextFormat.PlainText`.

    Please note that the insertTextFormat doesn't apply to
    `additionalTextEdits`. """
    insertTextMode: NotRequired["InsertTextMode"]
    """ How whitespace and indentation is handled during completion
    item insertion. If not provided the clients default value depends on
    the `textDocument.completion.insertTextMode` client capability.

    @since 3.16.0 """
    textEdit: NotRequired[Union["TextEdit", "InsertReplaceEdit"]]
    """ An {@link TextEdit edit} which is applied to a document when selecting
    this completion. When an edit is provided the value of
    {@link CompletionItem.insertText insertText} is ignored.

    Most editors support two different operations when accepting a completion
    item. One is to insert a completion text and the other is to replace an
    existing text with a completion text. Since this can usually not be
    predetermined by a server it can report both ranges. Clients need to
    signal support for `InsertReplaceEdits` via the
    `textDocument.completion.insertReplaceSupport` client capability
    property.

    *Note 1:* The text edit's range as well as both ranges from an insert
    replace edit must be a [single line] and they must contain the position
    at which completion has been requested.
    *Note 2:* If an `InsertReplaceEdit` is returned the edit's insert range
    must be a prefix of the edit's replace range, that means it must be
    contained and starting at the same position.

    @since 3.16.0 additional type `InsertReplaceEdit` """
    textEditText: NotRequired[str]
    """ The edit text used if the completion item is part of a CompletionList and
    CompletionList defines an item default for the text edit range.

    Clients will only honor this property if they opt into completion list
    item defaults using the capability `completionList.itemDefaults`.

    If not provided and a list's default range is provided the label
    property is used as a text.

    @since 3.17.0 """
    additionalTextEdits: NotRequired[List["TextEdit"]]
    """ An optional array of additional {@link TextEdit text edits} that are applied when
    selecting this completion. Edits must not overlap (including the same insert position)
    with the main {@link CompletionItem.textEdit edit} nor with themselves.

    Additional text edits should be used to change text unrelated to the current cursor position
    (for example adding an import statement at the top of the file if the completion item will
    insert an unqualified type). """
    commitCharacters: NotRequired[List[str]]
    """ An optional set of characters that when pressed while this completion is active will accept it first and
    then type that character. *Note* that all commit characters should have `length=1` and that superfluous
    characters will be ignored. """
    command: NotRequired["Command"]
    """ An optional {@link Command command} that is executed *after* inserting this completion. *Note* that
    additional modifications to the current document should be described with the
    {@link CompletionItem.additionalTextEdits additionalTextEdits}-property. """
    data: NotRequired["LSPAny"]
    """ A data entry field that is preserved on a completion item between a
    {@link CompletionRequest} and a {@link CompletionResolveRequest}. """


class CompletionList(TypedDict):
    """Represents a collection of {@link CompletionItem completion items} to be presented
    in the editor."""

    isIncomplete: bool
    """ This list it not complete. Further typing results in recomputing this list.

    Recomputed lists have all their items replaced (not appended) in the
    incomplete completion sessions. """
    itemDefaults: NotRequired["__CompletionList_itemDefaults_Type_1"]
    """ In many cases the items of an actual completion result share the same
    value for properties like `commitCharacters` or the range of a text
    edit. A completion list can therefore define item defaults which will
    be used if a completion item itself doesn't specify the value.

    If a completion list specifies a default value and a completion item
    also specifies a corresponding value the one from the item is used.

    Servers are only allowed to return default values if the client
    signals support for this via the `completionList.itemDefaults`
    capability.

    @since 3.17.0 """
    items: List["CompletionItem"]
    """ The completion items. """


class CompletionRegistrationOptions(TypedDict):
    """Registration options for a {@link CompletionRequest}."""

    documentSelector: Union["DocumentSelector", None]
    """ A document selector to identify the scope of the registration. If set to null
    the document selector provided on the client side will be used. """
    triggerCharacters: NotRequired[List[str]]
    """ Most tools trigger completion request automatically without explicitly requesting
    it using a keyboard shortcut (e.g. Ctrl+Space). Typically they do so when the user
    starts to type an identifier. For example if the user types `c` in a JavaScript file
    code complete will automatically pop up present `console` besides others as a
    completion item. Characters that make up identifiers don't need to be listed here.

    If code complete should automatically be trigger on characters not being valid inside
    an identifier (for example `.` in JavaScript) list them in `triggerCharacters`. """
    allCommitCharacters: NotRequired[List[str]]
    """ The list of all possible characters that commit a completion. This field can be used
    if clients don't support individual commit characters per completion item. See
    `ClientCapabilities.textDocument.completion.completionItem.commitCharactersSupport`

    If a server provides both `allCommitCharacters` and commit characters on an individual
    completion item the ones on the completion item win.

    @since 3.2.0 """
    resolveProvider: NotRequired[bool]
    """ The server provides support to resolve additional
    information for a completion item. """
    completionItem: NotRequired["__CompletionOptions_completionItem_Type_1"]
    """ The server supports the following `CompletionItem` specific
    capabilities.

    @since 3.17.0 """


class HoverParams(TypedDict):
    """Parameters for a {@link HoverRequest}."""

    textDocument: "TextDocumentIdentifier"
    """ The text document. """
    position: "Position"
    """ The position inside the text document. """
    workDoneToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report work done progress. """


class Hover(TypedDict):
    """The result of a hover request."""

    contents: Union["MarkupContent", "MarkedString", List["MarkedString"]]
    """ The hover's content """
    range: NotRequired["Range"]
    """ An optional range inside the text document that is used to
    visualize the hover, e.g. by changing the background color. """


class HoverRegistrationOptions(TypedDict):
    """Registration options for a {@link HoverRequest}."""

    documentSelector: Union["DocumentSelector", None]
    """ A document selector to identify the scope of the registration. If set to null
    the document selector provided on the client side will be used. """


class SignatureHelpParams(TypedDict):
    """Parameters for a {@link SignatureHelpRequest}."""

    context: NotRequired["SignatureHelpContext"]
    """ The signature help context. This is only available if the client specifies
    to send this using the client capability `textDocument.signatureHelp.contextSupport === true`

    @since 3.15.0 """
    textDocument: "TextDocumentIdentifier"
    """ The text document. """
    position: "Position"
    """ The position inside the text document. """
    workDoneToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report work done progress. """


class SignatureHelp(TypedDict):
    """Signature help represents the signature of something
    callable. There can be multiple signature but only one
    active and only one active parameter."""

    signatures: List["SignatureInformation"]
    """ One or more signatures. """
    activeSignature: NotRequired[Uint]
    """ The active signature. If omitted or the value lies outside the
    range of `signatures` the value defaults to zero or is ignored if
    the `SignatureHelp` has no signatures.

    Whenever possible implementors should make an active decision about
    the active signature and shouldn't rely on a default value.

    In future version of the protocol this property might become
    mandatory to better express this. """
    activeParameter: NotRequired[Uint]
    """ The active parameter of the active signature. If omitted or the value
    lies outside the range of `signatures[activeSignature].parameters`
    defaults to 0 if the active signature has parameters. If
    the active signature has no parameters it is ignored.
    In future version of the protocol this property might become
    mandatory to better express the active parameter if the
    active signature does have any. """


class SignatureHelpRegistrationOptions(TypedDict):
    """Registration options for a {@link SignatureHelpRequest}."""

    documentSelector: Union["DocumentSelector", None]
    """ A document selector to identify the scope of the registration. If set to null
    the document selector provided on the client side will be used. """
    triggerCharacters: NotRequired[List[str]]
    """ List of characters that trigger signature help automatically. """
    retriggerCharacters: NotRequired[List[str]]
    """ List of characters that re-trigger signature help.

    These trigger characters are only active when signature help is already showing. All trigger characters
    are also counted as re-trigger characters.

    @since 3.15.0 """


class DefinitionParams(TypedDict):
    """Parameters for a {@link DefinitionRequest}."""

    textDocument: "TextDocumentIdentifier"
    """ The text document. """
    position: "Position"
    """ The position inside the text document. """
    workDoneToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report work done progress. """
    partialResultToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report partial results (e.g. streaming) to
    the client. """


class DefinitionRegistrationOptions(TypedDict):
    """Registration options for a {@link DefinitionRequest}."""

    documentSelector: Union["DocumentSelector", None]
    """ A document selector to identify the scope of the registration. If set to null
    the document selector provided on the client side will be used. """


class ReferenceParams(TypedDict):
    """Parameters for a {@link ReferencesRequest}."""

    context: "ReferenceContext"
    textDocument: "TextDocumentIdentifier"
    """ The text document. """
    position: "Position"
    """ The position inside the text document. """
    workDoneToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report work done progress. """
    partialResultToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report partial results (e.g. streaming) to
    the client. """


class ReferenceRegistrationOptions(TypedDict):
    """Registration options for a {@link ReferencesRequest}."""

    documentSelector: Union["DocumentSelector", None]
    """ A document selector to identify the scope of the registration. If set to null
    the document selector provided on the client side will be used. """


class DocumentHighlightParams(TypedDict):
    """Parameters for a {@link DocumentHighlightRequest}."""

    textDocument: "TextDocumentIdentifier"
    """ The text document. """
    position: "Position"
    """ The position inside the text document. """
    workDoneToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report work done progress. """
    partialResultToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report partial results (e.g. streaming) to
    the client. """


class DocumentHighlight(TypedDict):
    """A document highlight is a range inside a text document which deserves
    special attention. Usually a document highlight is visualized by changing
    the background color of its range."""

    range: "Range"
    """ The range this highlight applies to. """
    kind: NotRequired["DocumentHighlightKind"]
    """ The highlight kind, default is {@link DocumentHighlightKind.Text text}. """


class DocumentHighlightRegistrationOptions(TypedDict):
    """Registration options for a {@link DocumentHighlightRequest}."""

    documentSelector: Union["DocumentSelector", None]
    """ A document selector to identify the scope of the registration. If set to null
    the document selector provided on the client side will be used. """


class DocumentSymbolParams(TypedDict):
    """Parameters for a {@link DocumentSymbolRequest}."""

    textDocument: "TextDocumentIdentifier"
    """ The text document. """
    workDoneToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report work done progress. """
    partialResultToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report partial results (e.g. streaming) to
    the client. """


class SymbolInformation(TypedDict):
    """Represents information about programming constructs like variables, classes,
    interfaces etc."""

    deprecated: NotRequired[bool]
    """ Indicates if this symbol is deprecated.

    @deprecated Use tags instead """
    location: "Location"
    """ The location of this symbol. The location's range is used by a tool
    to reveal the location in the editor. If the symbol is selected in the
    tool the range's start information is used to position the cursor. So
    the range usually spans more than the actual symbol's name and does
    normally include things like visibility modifiers.

    The range doesn't have to denote a node range in the sense of an abstract
    syntax tree. It can therefore not be used to re-construct a hierarchy of
    the symbols. """
    name: str
    """ The name of this symbol. """
    kind: "SymbolKind"
    """ The kind of this symbol. """
    tags: NotRequired[List["SymbolTag"]]
    """ Tags for this symbol.

    @since 3.16.0 """
    containerName: NotRequired[str]
    """ The name of the symbol containing this symbol. This information is for
    user interface purposes (e.g. to render a qualifier in the user interface
    if necessary). It can't be used to re-infer a hierarchy for the document
    symbols. """


class DocumentSymbol(TypedDict):
    """Represents programming constructs like variables, classes, interfaces etc.
    that appear in a document. Document symbols can be hierarchical and they
    have two ranges: one that encloses its definition and one that points to
    its most interesting range, e.g. the range of an identifier."""

    name: str
    """ The name of this symbol. Will be displayed in the user interface and therefore must not be
    an empty string or a string only consisting of white spaces. """
    detail: NotRequired[str]
    """ More detail for this symbol, e.g the signature of a function. """
    kind: "SymbolKind"
    """ The kind of this symbol. """
    tags: NotRequired[List["SymbolTag"]]
    """ Tags for this document symbol.

    @since 3.16.0 """
    deprecated: NotRequired[bool]
    """ Indicates if this symbol is deprecated.

    @deprecated Use tags instead """
    range: "Range"
    """ The range enclosing this symbol not including leading/trailing whitespace but everything else
    like comments. This information is typically used to determine if the clients cursor is
    inside the symbol to reveal in the symbol in the UI. """
    selectionRange: "Range"
    """ The range that should be selected and revealed when this symbol is being picked, e.g the name of a function.
    Must be contained by the `range`. """
    children: NotRequired[List["DocumentSymbol"]]
    """ Children of this symbol, e.g. properties of a class. """


class DocumentSymbolRegistrationOptions(TypedDict):
    """Registration options for a {@link DocumentSymbolRequest}."""

    documentSelector: Union["DocumentSelector", None]
    """ A document selector to identify the scope of the registration. If set to null
    the document selector provided on the client side will be used. """
    label: NotRequired[str]
    """ A human-readable string that is shown when multiple outlines trees
    are shown for the same document.

    @since 3.16.0 """


class CodeActionParams(TypedDict):
    """The parameters of a {@link CodeActionRequest}."""

    textDocument: "TextDocumentIdentifier"
    """ The document in which the command was invoked. """
    range: "Range"
    """ The range for which the command was invoked. """
    context: "CodeActionContext"
    """ Context carrying additional information. """
    workDoneToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report work done progress. """
    partialResultToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report partial results (e.g. streaming) to
    the client. """


class Command(TypedDict):
    """Represents a reference to a command. Provides a title which
    will be used to represent a command in the UI and, optionally,
    an array of arguments which will be passed to the command handler
    function when invoked."""

    title: str
    """ Title of the command, like `save`. """
    command: str
    """ The identifier of the actual command handler. """
    arguments: NotRequired[List["LSPAny"]]
    """ Arguments that the command handler should be
    invoked with. """


class CodeAction(TypedDict):
    """A code action represents a change that can be performed in code, e.g. to fix a problem or
    to refactor code.

    A CodeAction must set either `edit` and/or a `command`. If both are supplied, the `edit` is applied first, then the `command` is executed.
    """

    title: str
    """ A short, human-readable, title for this code action. """
    kind: NotRequired["CodeActionKind"]
    """ The kind of the code action.

    Used to filter code actions. """
    diagnostics: NotRequired[List["Diagnostic"]]
    """ The diagnostics that this code action resolves. """
    isPreferred: NotRequired[bool]
    """ Marks this as a preferred action. Preferred actions are used by the `auto fix` command and can be targeted
    by keybindings.

    A quick fix should be marked preferred if it properly addresses the underlying error.
    A refactoring should be marked preferred if it is the most reasonable choice of actions to take.

    @since 3.15.0 """
    disabled: NotRequired["__CodeAction_disabled_Type_1"]
    """ Marks that the code action cannot currently be applied.

    Clients should follow the following guidelines regarding disabled code actions:

      - Disabled code actions are not shown in automatic [lightbulbs](https://code.visualstudio.com/docs/editor/editingevolved#_code-action)
        code action menus.

      - Disabled actions are shown as faded out in the code action menu when the user requests a more specific type
        of code action, such as refactorings.

      - If the user has a [keybinding](https://code.visualstudio.com/docs/editor/refactoring#_keybindings-for-code-actions)
        that auto applies a code action and only disabled code actions are returned, the client should show the user an
        error message with `reason` in the editor.

    @since 3.16.0 """
    edit: NotRequired["WorkspaceEdit"]
    """ The workspace edit this code action performs. """
    command: NotRequired["Command"]
    """ A command this code action executes. If a code action
    provides an edit and a command, first the edit is
    executed and then the command. """
    data: NotRequired["LSPAny"]
    """ A data entry field that is preserved on a code action between
    a `textDocument/codeAction` and a `codeAction/resolve` request.

    @since 3.16.0 """


class CodeActionRegistrationOptions(TypedDict):
    """Registration options for a {@link CodeActionRequest}."""

    documentSelector: Union["DocumentSelector", None]
    """ A document selector to identify the scope of the registration. If set to null
    the document selector provided on the client side will be used. """
    codeActionKinds: NotRequired[List["CodeActionKind"]]
    """ CodeActionKinds that this server may return.

    The list of kinds may be generic, such as `CodeActionKind.Refactor`, or the server
    may list out every specific kind they provide. """
    resolveProvider: NotRequired[bool]
    """ The server provides support to resolve additional
    information for a code action.

    @since 3.16.0 """


class WorkspaceSymbolParams(TypedDict):
    """The parameters of a {@link WorkspaceSymbolRequest}."""

    query: str
    """ A query string to filter symbols by. Clients may send an empty
    string here to request all symbols. """
    workDoneToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report work done progress. """
    partialResultToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report partial results (e.g. streaming) to
    the client. """


class WorkspaceSymbol(TypedDict):
    """A special workspace symbol that supports locations without a range.

    See also SymbolInformation.

    @since 3.17.0"""

    location: Union["Location", "__WorkspaceSymbol_location_Type_1"]
    """ The location of the symbol. Whether a server is allowed to
    return a location without a range depends on the client
    capability `workspace.symbol.resolveSupport`.

    See SymbolInformation#location for more details. """
    data: NotRequired["LSPAny"]
    """ A data entry field that is preserved on a workspace symbol between a
    workspace symbol request and a workspace symbol resolve request. """
    name: str
    """ The name of this symbol. """
    kind: "SymbolKind"
    """ The kind of this symbol. """
    tags: NotRequired[List["SymbolTag"]]
    """ Tags for this symbol.

    @since 3.16.0 """
    containerName: NotRequired[str]
    """ The name of the symbol containing this symbol. This information is for
    user interface purposes (e.g. to render a qualifier in the user interface
    if necessary). It can't be used to re-infer a hierarchy for the document
    symbols. """


class WorkspaceSymbolRegistrationOptions(TypedDict):
    """Registration options for a {@link WorkspaceSymbolRequest}."""

    resolveProvider: NotRequired[bool]
    """ The server provides support to resolve additional
    information for a workspace symbol.

    @since 3.17.0 """


class CodeLensParams(TypedDict):
    """The parameters of a {@link CodeLensRequest}."""

    textDocument: "TextDocumentIdentifier"
    """ The document to request code lens for. """
    workDoneToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report work done progress. """
    partialResultToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report partial results (e.g. streaming) to
    the client. """


class CodeLens(TypedDict):
    """A code lens represents a {@link Command command} that should be shown along with
    source text, like the number of references, a way to run tests, etc.

    A code lens is _unresolved_ when no command is associated to it. For performance
    reasons the creation of a code lens and resolving should be done in two stages."""

    range: "Range"
    """ The range in which this code lens is valid. Should only span a single line. """
    command: NotRequired["Command"]
    """ The command this code lens represents. """
    data: NotRequired["LSPAny"]
    """ A data entry field that is preserved on a code lens item between
    a {@link CodeLensRequest} and a [CodeLensResolveRequest]
    (#CodeLensResolveRequest) """


class CodeLensRegistrationOptions(TypedDict):
    """Registration options for a {@link CodeLensRequest}."""

    documentSelector: Union["DocumentSelector", None]
    """ A document selector to identify the scope of the registration. If set to null
    the document selector provided on the client side will be used. """
    resolveProvider: NotRequired[bool]
    """ Code lens has a resolve provider as well. """


class DocumentLinkParams(TypedDict):
    """The parameters of a {@link DocumentLinkRequest}."""

    textDocument: "TextDocumentIdentifier"
    """ The document to provide document links for. """
    workDoneToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report work done progress. """
    partialResultToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report partial results (e.g. streaming) to
    the client. """


class DocumentLink(TypedDict):
    """A document link is a range in a text document that links to an internal or external resource, like another
    text document or a web site."""

    range: "Range"
    """ The range this link applies to. """
    target: NotRequired[str]
    """ The uri this link points to. If missing a resolve request is sent later. """
    tooltip: NotRequired[str]
    """ The tooltip text when you hover over this link.

    If a tooltip is provided, is will be displayed in a string that includes instructions on how to
    trigger the link, such as `{0} (ctrl + click)`. The specific instructions vary depending on OS,
    user settings, and localization.

    @since 3.15.0 """
    data: NotRequired["LSPAny"]
    """ A data entry field that is preserved on a document link between a
    DocumentLinkRequest and a DocumentLinkResolveRequest. """


class DocumentLinkRegistrationOptions(TypedDict):
    """Registration options for a {@link DocumentLinkRequest}."""

    documentSelector: Union["DocumentSelector", None]
    """ A document selector to identify the scope of the registration. If set to null
    the document selector provided on the client side will be used. """
    resolveProvider: NotRequired[bool]
    """ Document links have a resolve provider as well. """


class DocumentFormattingParams(TypedDict):
    """The parameters of a {@link DocumentFormattingRequest}."""

    textDocument: "TextDocumentIdentifier"
    """ The document to format. """
    options: "FormattingOptions"
    """ The format options. """
    workDoneToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report work done progress. """


class DocumentFormattingRegistrationOptions(TypedDict):
    """Registration options for a {@link DocumentFormattingRequest}."""

    documentSelector: Union["DocumentSelector", None]
    """ A document selector to identify the scope of the registration. If set to null
    the document selector provided on the client side will be used. """


class DocumentRangeFormattingParams(TypedDict):
    """The parameters of a {@link DocumentRangeFormattingRequest}."""

    textDocument: "TextDocumentIdentifier"
    """ The document to format. """
    range: "Range"
    """ The range to format """
    options: "FormattingOptions"
    """ The format options """
    workDoneToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report work done progress. """


class DocumentRangeFormattingRegistrationOptions(TypedDict):
    """Registration options for a {@link DocumentRangeFormattingRequest}."""

    documentSelector: Union["DocumentSelector", None]
    """ A document selector to identify the scope of the registration. If set to null
    the document selector provided on the client side will be used. """


class DocumentOnTypeFormattingParams(TypedDict):
    """The parameters of a {@link DocumentOnTypeFormattingRequest}."""

    textDocument: "TextDocumentIdentifier"
    """ The document to format. """
    position: "Position"
    """ The position around which the on type formatting should happen.
    This is not necessarily the exact position where the character denoted
    by the property `ch` got typed. """
    ch: str
    """ The character that has been typed that triggered the formatting
    on type request. That is not necessarily the last character that
    got inserted into the document since the client could auto insert
    characters as well (e.g. like automatic brace completion). """
    options: "FormattingOptions"
    """ The formatting options. """


class DocumentOnTypeFormattingRegistrationOptions(TypedDict):
    """Registration options for a {@link DocumentOnTypeFormattingRequest}."""

    documentSelector: Union["DocumentSelector", None]
    """ A document selector to identify the scope of the registration. If set to null
    the document selector provided on the client side will be used. """
    firstTriggerCharacter: str
    """ A character on which formatting should be triggered, like `{`. """
    moreTriggerCharacter: NotRequired[List[str]]
    """ More trigger characters. """


class RenameParams(TypedDict):
    """The parameters of a {@link RenameRequest}."""

    textDocument: "TextDocumentIdentifier"
    """ The document to rename. """
    position: "Position"
    """ The position at which this request was sent. """
    newName: str
    """ The new name of the symbol. If the given name is not valid the
    request must return a {@link ResponseError} with an
    appropriate message set. """
    workDoneToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report work done progress. """


class RenameRegistrationOptions(TypedDict):
    """Registration options for a {@link RenameRequest}."""

    documentSelector: Union["DocumentSelector", None]
    """ A document selector to identify the scope of the registration. If set to null
    the document selector provided on the client side will be used. """
    prepareProvider: NotRequired[bool]
    """ Renames should be checked and tested before being executed.

    @since version 3.12.0 """


class PrepareRenameParams(TypedDict):
    textDocument: "TextDocumentIdentifier"
    """ The text document. """
    position: "Position"
    """ The position inside the text document. """
    workDoneToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report work done progress. """


class ExecuteCommandParams(TypedDict):
    """The parameters of a {@link ExecuteCommandRequest}."""

    command: str
    """ The identifier of the actual command handler. """
    arguments: NotRequired[List["LSPAny"]]
    """ Arguments that the command should be invoked with. """
    workDoneToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report work done progress. """


class ExecuteCommandRegistrationOptions(TypedDict):
    """Registration options for a {@link ExecuteCommandRequest}."""

    commands: List[str]
    """ The commands to be executed on the server """


class ApplyWorkspaceEditParams(TypedDict):
    """The parameters passed via a apply workspace edit request."""

    label: NotRequired[str]
    """ An optional label of the workspace edit. This label is
    presented in the user interface for example on an undo
    stack to undo the workspace edit. """
    edit: "WorkspaceEdit"
    """ The edits to apply. """


class ApplyWorkspaceEditResult(TypedDict):
    """The result returned from the apply workspace edit request.

    @since 3.17 renamed from ApplyWorkspaceEditResponse"""

    applied: bool
    """ Indicates whether the edit was applied or not. """
    failureReason: NotRequired[str]
    """ An optional textual description for why the edit was not applied.
    This may be used by the server for diagnostic logging or to provide
    a suitable error for a request that triggered the edit. """
    failedChange: NotRequired[Uint]
    """ Depending on the client's failure handling strategy `failedChange` might
    contain the index of the change that failed. This property is only available
    if the client signals a `failureHandlingStrategy` in its client capabilities. """


class WorkDoneProgressBegin(TypedDict):
    kind: Literal["begin"]
    title: str
    """ Mandatory title of the progress operation. Used to briefly inform about
    the kind of operation being performed.

    Examples: "Indexing" or "Linking dependencies". """
    cancellable: NotRequired[bool]
    """ Controls if a cancel button should show to allow the user to cancel the
    long running operation. Clients that don't support cancellation are allowed
    to ignore the setting. """
    message: NotRequired[str]
    """ Optional, more detailed associated progress message. Contains
    complementary information to the `title`.

    Examples: "3/25 files", "project/src/module2", "node_modules/some_dep".
    If unset, the previous progress message (if any) is still valid. """
    percentage: NotRequired[Uint]
    """ Optional progress percentage to display (value 100 is considered 100%).
    If not provided infinite progress is assumed and clients are allowed
    to ignore the `percentage` value in subsequent in report notifications.

    The value should be steadily rising. Clients are free to ignore values
    that are not following this rule. The value range is [0, 100]. """


class WorkDoneProgressReport(TypedDict):
    kind: Literal["report"]
    cancellable: NotRequired[bool]
    """ Controls enablement state of a cancel button.

    Clients that don't support cancellation or don't support controlling the button's
    enablement state are allowed to ignore the property. """
    message: NotRequired[str]
    """ Optional, more detailed associated progress message. Contains
    complementary information to the `title`.

    Examples: "3/25 files", "project/src/module2", "node_modules/some_dep".
    If unset, the previous progress message (if any) is still valid. """
    percentage: NotRequired[Uint]
    """ Optional progress percentage to display (value 100 is considered 100%).
    If not provided infinite progress is assumed and clients are allowed
    to ignore the `percentage` value in subsequent in report notifications.

    The value should be steadily rising. Clients are free to ignore values
    that are not following this rule. The value range is [0, 100] """


class WorkDoneProgressEnd(TypedDict):
    kind: Literal["end"]
    message: NotRequired[str]
    """ Optional, a final message indicating to for example indicate the outcome
    of the operation. """


class SetTraceParams(TypedDict):
    value: "TraceValues"


class LogTraceParams(TypedDict):
    message: str
    verbose: NotRequired[str]


class CancelParams(TypedDict):
    id: Union[int, str]
    """ The request id to cancel. """


class ProgressParams(TypedDict):
    token: "ProgressToken"
    """ The progress token provided by the client or server. """
    value: "LSPAny"
    """ The progress data. """


class TextDocumentPositionParams(TypedDict):
    """A parameter literal used in requests to pass a text document and a position inside that
    document."""

    textDocument: "TextDocumentIdentifier"
    """ The text document. """
    position: "Position"
    """ The position inside the text document. """


class WorkDoneProgressParams(TypedDict):
    workDoneToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report work done progress. """


class PartialResultParams(TypedDict):
    partialResultToken: NotRequired["ProgressToken"]
    """ An optional token that a server can use to report partial results (e.g. streaming) to
    the client. """


class LocationLink(TypedDict):
    """Represents the connection of two locations. Provides additional metadata over normal {@link Location locations},
    including an origin range."""

    originSelectionRange: NotRequired["Range"]
    """ Span of the origin of this link.

    Used as the underlined span for mouse interaction. Defaults to the word range at
    the definition position. """
    targetUri: "DocumentUri"
    """ The target resource identifier of this link. """
    targetRange: "Range"
    """ The full target range of this link. If the target for example is a symbol then target range is the
    range enclosing this symbol not including leading/trailing whitespace but everything else
    like comments. This information is typically used to highlight the range in the editor. """
    targetSelectionRange: "Range"
    """ The range that should be selected and revealed when this link is being followed, e.g the name of a function.
    Must be contained by the `targetRange`. See also `DocumentSymbol#range` """


class Range(TypedDict):
    """A range in a text document expressed as (zero-based) start and end positions.

    If you want to specify a range that contains a line including the line ending
    character(s) then use an end position denoting the start of the next line.
    For example:
    ```ts
    {
        start: { line: 5, character: 23 }
        end : { line 6, character : 0 }
    }
    ```"""

    start: "Position"
    """ The range's start position. """
    end: "Position"
    """ The range's end position. """


class ImplementationOptions(TypedDict):
    workDoneProgress: NotRequired[bool]


class StaticRegistrationOptions(TypedDict):
    """Static registration options to be returned in the initialize
    request."""

    id: NotRequired[str]
    """ The id used to register the request. The id can be used to deregister
    the request again. See also Registration#id. """


class TypeDefinitionOptions(TypedDict):
    workDoneProgress: NotRequired[bool]


class WorkspaceFoldersChangeEvent(TypedDict):
    """The workspace folder change event."""

    added: List["WorkspaceFolder"]
    """ The array of added workspace folders """
    removed: List["WorkspaceFolder"]
    """ The array of the removed workspace folders """


class ConfigurationItem(TypedDict):
    scopeUri: NotRequired[str]
    """ The scope to get the configuration section for. """
    section: NotRequired[str]
    """ The configuration section asked for. """


class TextDocumentIdentifier(TypedDict):
    """A literal to identify a text document in the client."""

    uri: "DocumentUri"
    """ The text document's uri. """


class Color(TypedDict):
    """Represents a color in RGBA space."""

    red: float
    """ The red component of this color in the range [0-1]. """
    green: float
    """ The green component of this color in the range [0-1]. """
    blue: float
    """ The blue component of this color in the range [0-1]. """
    alpha: float
    """ The alpha component of this color in the range [0-1]. """


class DocumentColorOptions(TypedDict):
    workDoneProgress: NotRequired[bool]


class FoldingRangeOptions(TypedDict):
    workDoneProgress: NotRequired[bool]


class DeclarationOptions(TypedDict):
    workDoneProgress: NotRequired[bool]


class Position(TypedDict):
    """Position in a text document expressed as zero-based line and character
    offset. Prior to 3.17 the offsets were always based on a UTF-16 string
    representation. So a string of the form `a𐐀b` the character offset of the
    character `a` is 0, the character offset of `𐐀` is 1 and the character
    offset of b is 3 since `𐐀` is represented using two code units in UTF-16.
    Since 3.17 clients and servers can agree on a different string encoding
    representation (e.g. UTF-8). The client announces it's supported encoding
    via the client capability [`general.positionEncodings`](#clientCapabilities).
    The value is an array of position encodings the client supports, with
    decreasing preference (e.g. the encoding at index `0` is the most preferred
    one). To stay backwards compatible the only mandatory encoding is UTF-16
    represented via the string `utf-16`. The server can pick one of the
    encodings offered by the client and signals that encoding back to the
    client via the initialize result's property
    [`capabilities.positionEncoding`](#serverCapabilities). If the string value
    `utf-16` is missing from the client's capability `general.positionEncodings`
    servers can safely assume that the client supports UTF-16. If the server
    omits the position encoding in its initialize result the encoding defaults
    to the string value `utf-16`. Implementation considerations: since the
    conversion from one encoding into another requires the content of the
    file / line the conversion is best done where the file is read which is
    usually on the server side.

    Positions are line end character agnostic. So you can not specify a position
    that denotes `\r|\n` or `\n|` where `|` represents the character offset.

    @since 3.17.0 - support for negotiated position encoding."""

    line: Uint
    """ Line position in a document (zero-based).

    If a line number is greater than the number of lines in a document, it defaults back to the number of lines in the document.
    If a line number is negative, it defaults to 0. """
    character: Uint
    """ Character offset on a line in a document (zero-based).

    The meaning of this offset is determined by the negotiated
    `PositionEncodingKind`.

    If the character value is greater than the line length it defaults back to the
    line length. """


class SelectionRangeOptions(TypedDict):
    workDoneProgress: NotRequired[bool]


class CallHierarchyOptions(TypedDict):
    """Call hierarchy options used during static registration.

    @since 3.16.0"""

    workDoneProgress: NotRequired[bool]


class SemanticTokensOptions(TypedDict):
    """@since 3.16.0"""

    legend: "SemanticTokensLegend"
    """ The legend used by the server """
    range: NotRequired[Union[bool, dict]]
    """ Server supports providing semantic tokens for a specific range
    of a document. """
    full: NotRequired[Union[bool, "__SemanticTokensOptions_full_Type_2"]]
    """ Server supports providing semantic tokens for a full document. """
    workDoneProgress: NotRequired[bool]


class SemanticTokensEdit(TypedDict):
    """@since 3.16.0"""

    start: Uint
    """ The start offset of the edit. """
    deleteCount: Uint
    """ The count of elements to remove. """
    data: NotRequired[List[Uint]]
    """ The elements to insert. """


class LinkedEditingRangeOptions(TypedDict):
    workDoneProgress: NotRequired[bool]


class FileCreate(TypedDict):
    """Represents information on a file/folder create.

    @since 3.16.0"""

    uri: str
    """ A file:// URI for the location of the file/folder being created. """


class TextDocumentEdit(TypedDict):
    """Describes textual changes on a text document. A TextDocumentEdit describes all changes
    on a document version Si and after they are applied move the document to version Si+1.
    So the creator of a TextDocumentEdit doesn't need to sort the array of edits or do any
    kind of ordering. However the edits must be non overlapping."""

    textDocument: "OptionalVersionedTextDocumentIdentifier"
    """ The text document to change. """
    edits: List[Union["TextEdit", "AnnotatedTextEdit"]]
    """ The edits to be applied.

    @since 3.16.0 - support for AnnotatedTextEdit. This is guarded using a
    client capability. """


class CreateFile(TypedDict):
    """Create file operation."""

    kind: Literal["create"]
    """ A create """
    uri: "DocumentUri"
    """ The resource to create. """
    options: NotRequired["CreateFileOptions"]
    """ Additional options """
    annotationId: NotRequired["ChangeAnnotationIdentifier"]
    """ An optional annotation identifier describing the operation.

    @since 3.16.0 """


class RenameFile(TypedDict):
    """Rename file operation"""

    kind: Literal["rename"]
    """ A rename """
    oldUri: "DocumentUri"
    """ The old (existing) location. """
    newUri: "DocumentUri"
    """ The new location. """
    options: NotRequired["RenameFileOptions"]
    """ Rename options. """
    annotationId: NotRequired["ChangeAnnotationIdentifier"]
    """ An optional annotation identifier describing the operation.

    @since 3.16.0 """


class DeleteFile(TypedDict):
    """Delete file operation"""

    kind: Literal["delete"]
    """ A delete """
    uri: "DocumentUri"
    """ The file to delete. """
    options: NotRequired["DeleteFileOptions"]
    """ Delete options. """
    annotationId: NotRequired["ChangeAnnotationIdentifier"]
    """ An optional annotation identifier describing the operation.

    @since 3.16.0 """


class ChangeAnnotation(TypedDict):
    """Additional information that describes document changes.

    @since 3.16.0"""

    label: str
    """ A human-readable string describing the actual change. The string
    is rendered prominent in the user interface. """
    needsConfirmation: NotRequired[bool]
    """ A flag which indicates that user confirmation is needed
    before applying the change. """
    description: NotRequired[str]
    """ A human-readable string which is rendered less prominent in
    the user interface. """


class FileOperationFilter(TypedDict):
    """A filter to describe in which file operation requests or notifications
    the server is interested in receiving.

    @since 3.16.0"""

    scheme: NotRequired[str]
    """ A Uri scheme like `file` or `untitled`. """
    pattern: "FileOperationPattern"
    """ The actual file operation pattern. """


class FileRename(TypedDict):
    """Represents information on a file/folder rename.

    @since 3.16.0"""

    oldUri: str
    """ A file:// URI for the original location of the file/folder being renamed. """
    newUri: str
    """ A file:// URI for the new location of the file/folder being renamed. """


class FileDelete(TypedDict):
    """Represents information on a file/folder delete.

    @since 3.16.0"""

    uri: str
    """ A file:// URI for the location of the file/folder being deleted. """


class MonikerOptions(TypedDict):
    workDoneProgress: NotRequired[bool]


class TypeHierarchyOptions(TypedDict):
    """Type hierarchy options used during static registration.

    @since 3.17.0"""

    workDoneProgress: NotRequired[bool]


class InlineValueContext(TypedDict):
    """@since 3.17.0"""

    frameId: int
    """ The stack frame (as a DAP Id) where the execution has stopped. """
    stoppedLocation: "Range"
    """ The document range where execution has stopped.
    Typically the end position of the range denotes the line where the inline values are shown. """


class InlineValueText(TypedDict):
    """Provide inline value as text.

    @since 3.17.0"""

    range: "Range"
    """ The document range for which the inline value applies. """
    text: str
    """ The text of the inline value. """


class InlineValueVariableLookup(TypedDict):
    """Provide inline value through a variable lookup.
    If only a range is specified, the variable name will be extracted from the underlying document.
    An optional variable name can be used to override the extracted name.

    @since 3.17.0"""

    range: "Range"
    """ The document range for which the inline value applies.
    The range is used to extract the variable name from the underlying document. """
    variableName: NotRequired[str]
    """ If specified the name of the variable to look up. """
    caseSensitiveLookup: bool
    """ How to perform the lookup. """


class InlineValueEvaluatableExpression(TypedDict):
    """Provide an inline value through an expression evaluation.
    If only a range is specified, the expression will be extracted from the underlying document.
    An optional expression can be used to override the extracted expression.

    @since 3.17.0"""

    range: "Range"
    """ The document range for which the inline value applies.
    The range is used to extract the evaluatable expression from the underlying document. """
    expression: NotRequired[str]
    """ If specified the expression overrides the extracted expression. """


class InlineValueOptions(TypedDict):
    """Inline value options used during static registration.

    @since 3.17.0"""

    workDoneProgress: NotRequired[bool]


class InlayHintLabelPart(TypedDict):
    """An inlay hint label part allows for interactive and composite labels
    of inlay hints.

    @since 3.17.0"""

    value: str
    """ The value of this label part. """
    tooltip: NotRequired[Union[str, "MarkupContent"]]
    """ The tooltip text when you hover over this label part. Depending on
    the client capability `inlayHint.resolveSupport` clients might resolve
    this property late using the resolve request. """
    location: NotRequired["Location"]
    """ An optional source code location that represents this
    label part.

    The editor will use this location for the hover and for code navigation
    features: This part will become a clickable link that resolves to the
    definition of the symbol at the given location (not necessarily the
    location itself), it shows the hover that shows at the given location,
    and it shows a context menu with further code navigation commands.

    Depending on the client capability `inlayHint.resolveSupport` clients
    might resolve this property late using the resolve request. """
    command: NotRequired["Command"]
    """ An optional command for this label part.

    Depending on the client capability `inlayHint.resolveSupport` clients
    might resolve this property late using the resolve request. """


class MarkupContent(TypedDict):
    """A `MarkupContent` literal represents a string value which content is interpreted base on its
    kind flag. Currently the protocol supports `plaintext` and `markdown` as markup kinds.

    If the kind is `markdown` then the value can contain fenced code blocks like in GitHub issues.
    See https://help.github.com/articles/creating-and-highlighting-code-blocks/#syntax-highlighting

    Here is an example how such a string can be constructed using JavaScript / TypeScript:
    ```ts
    let markdown: MarkdownContent = {
     kind: MarkupKind.Markdown,
     value: [
       '# Header',
       'Some text',
       '```typescript',
       'someCode();',
       '```'
     ].join('\n')
    };
    ```

    *Please Note* that clients might sanitize the return markdown. A client could decide to
    remove HTML from the markdown to avoid script execution."""

    kind: "MarkupKind"
    """ The type of the Markup """
    value: str
    """ The content itself """


class InlayHintOptions(TypedDict):
    """Inlay hint options used during static registration.

    @since 3.17.0"""

    resolveProvider: NotRequired[bool]
    """ The server provides support to resolve additional
    information for an inlay hint item. """
    workDoneProgress: NotRequired[bool]


class RelatedFullDocumentDiagnosticReport(TypedDict):
    """A full diagnostic report with a set of related documents.

    @since 3.17.0"""

    relatedDocuments: NotRequired[
        Dict[
            "DocumentUri",
            Union["FullDocumentDiagnosticReport", "UnchangedDocumentDiagnosticReport"],
        ]
    ]
    """ Diagnostics of related documents. This information is useful
    in programming languages where code in a file A can generate
    diagnostics in a file B which A depends on. An example of
    such a language is C/C++ where marco definitions in a file
    a.cpp and result in errors in a header file b.hpp.

    @since 3.17.0 """
    kind: Literal["full"]
    """ A full document diagnostic report. """
    resultId: NotRequired[str]
    """ An optional result id. If provided it will
    be sent on the next diagnostic request for the
    same document. """
    items: List["Diagnostic"]
    """ The actual items. """


class RelatedUnchangedDocumentDiagnosticReport(TypedDict):
    """An unchanged diagnostic report with a set of related documents.

    @since 3.17.0"""

    relatedDocuments: NotRequired[
        Dict[
            "DocumentUri",
            Union["FullDocumentDiagnosticReport", "UnchangedDocumentDiagnosticReport"],
        ]
    ]
    """ Diagnostics of related documents. This information is useful
    in programming languages where code in a file A can generate
    diagnostics in a file B which A depends on. An example of
    such a language is C/C++ where marco definitions in a file
    a.cpp and result in errors in a header file b.hpp.

    @since 3.17.0 """
    kind: Literal["unchanged"]
    """ A document diagnostic report indicating
    no changes to the last result. A server can
    only return `unchanged` if result ids are
    provided. """
    resultId: str
    """ A result id which will be sent on the next
    diagnostic request for the same document. """


class FullDocumentDiagnosticReport(TypedDict):
    """A diagnostic report with a full set of problems.

    @since 3.17.0"""

    kind: Literal["full"]
    """ A full document diagnostic report. """
    resultId: NotRequired[str]
    """ An optional result id. If provided it will
    be sent on the next diagnostic request for the
    same document. """
    items: List["Diagnostic"]
    """ The actual items. """


class UnchangedDocumentDiagnosticReport(TypedDict):
    """A diagnostic report indicating that the last returned
    report is still accurate.

    @since 3.17.0"""

    kind: Literal["unchanged"]
    """ A document diagnostic report indicating
    no changes to the last result. A server can
    only return `unchanged` if result ids are
    provided. """
    resultId: str
    """ A result id which will be sent on the next
    diagnostic request for the same document. """


class DiagnosticOptions(TypedDict):
    """Diagnostic options.

    @since 3.17.0"""

    identifier: NotRequired[str]
    """ An optional identifier under which the diagnostics are
    managed by the client. """
    interFileDependencies: bool
    """ Whether the language has inter file dependencies meaning that
    editing code in one file can result in a different diagnostic
    set in another file. Inter file dependencies are common for
    most programming languages and typically uncommon for linters. """
    workspaceDiagnostics: bool
    """ The server provides support for workspace diagnostics as well. """
    workDoneProgress: NotRequired[bool]


class PreviousResultId(TypedDict):
    """A previous result id in a workspace pull request.

    @since 3.17.0"""

    uri: "DocumentUri"
    """ The URI for which the client knowns a
    result id. """
    value: str
    """ The value of the previous result id. """


class NotebookDocument(TypedDict):
    """A notebook document.

    @since 3.17.0"""

    uri: "URI"
    """ The notebook document's uri. """
    notebookType: str
    """ The type of the notebook. """
    version: int
    """ The version number of this document (it will increase after each
    change, including undo/redo). """
    metadata: NotRequired["LSPObject"]
    """ Additional metadata stored with the notebook
    document.

    Note: should always be an object literal (e.g. LSPObject) """
    cells: List["NotebookCell"]
    """ The cells of a notebook. """


class TextDocumentItem(TypedDict):
    """An item to transfer a text document from the client to the
    server."""

    uri: "DocumentUri"
    """ The text document's uri. """
    languageId: str
    """ The text document's language identifier. """
    version: int
    """ The version number of this document (it will increase after each
    change, including undo/redo). """
    text: str
    """ The content of the opened text document. """


class VersionedNotebookDocumentIdentifier(TypedDict):
    """A versioned notebook document identifier.

    @since 3.17.0"""

    version: int
    """ The version number of this notebook document. """
    uri: "URI"
    """ The notebook document's uri. """


class NotebookDocumentChangeEvent(TypedDict):
    """A change event for a notebook document.

    @since 3.17.0"""

    metadata: NotRequired["LSPObject"]
    """ The changed meta data if any.

    Note: should always be an object literal (e.g. LSPObject) """
    cells: NotRequired["__NotebookDocumentChangeEvent_cells_Type_1"]
    """ Changes to cells """


class NotebookDocumentIdentifier(TypedDict):
    """A literal to identify a notebook document in the client.

    @since 3.17.0"""

    uri: "URI"
    """ The notebook document's uri. """


class Registration(TypedDict):
    """General parameters to to register for an notification or to register a provider."""

    id: str
    """ The id used to register the request. The id can be used to deregister
    the request again. """
    method: str
    """ The method / capability to register for. """
    registerOptions: NotRequired["LSPAny"]
    """ Options necessary for the registration. """


class Unregistration(TypedDict):
    """General parameters to unregister a request or notification."""

    id: str
    """ The id used to unregister the request or notification. Usually an id
    provided during the register request. """
    method: str
    """ The method to unregister for. """


class WorkspaceFoldersInitializeParams(TypedDict):
    workspaceFolders: NotRequired[Union[List["WorkspaceFolder"], None]]
    """ The workspace folders configured in the client when the server starts.

    This property is only available if the client supports workspace folders.
    It can be `null` if the client supports workspace folders but none are
    configured.

    @since 3.6.0 """


class ServerCapabilities(TypedDict):
    """Defines the capabilities provided by a language
    server."""

    positionEncoding: NotRequired["PositionEncodingKind"]
    """ The position encoding the server picked from the encodings offered
    by the client via the client capability `general.positionEncodings`.

    If the client didn't provide any position encodings the only valid
    value that a server can return is 'utf-16'.

    If omitted it defaults to 'utf-16'.

    @since 3.17.0 """
    textDocumentSync: NotRequired[Union["TextDocumentSyncOptions", "TextDocumentSyncKind"]]
    """ Defines how text documents are synced. Is either a detailed structure
    defining each notification or for backwards compatibility the
    TextDocumentSyncKind number. """
    notebookDocumentSync: NotRequired[Union["NotebookDocumentSyncOptions", "NotebookDocumentSyncRegistrationOptions"]]
    """ Defines how notebook documents are synced.

    @since 3.17.0 """
    completionProvider: NotRequired["CompletionOptions"]
    """ The server provides completion support. """
    hoverProvider: NotRequired[Union[bool, "HoverOptions"]]
    """ The server provides hover support. """
    signatureHelpProvider: NotRequired["SignatureHelpOptions"]
    """ The server provides signature help support. """
    declarationProvider: NotRequired[Union[bool, "DeclarationOptions", "DeclarationRegistrationOptions"]]
    """ The server provides Goto Declaration support. """
    definitionProvider: NotRequired[Union[bool, "DefinitionOptions"]]
    """ The server provides goto definition support. """
    typeDefinitionProvider: NotRequired[Union[bool, "TypeDefinitionOptions", "TypeDefinitionRegistrationOptions"]]
    """ The server provides Goto Type Definition support. """
    implementationProvider: NotRequired[Union[bool, "ImplementationOptions", "ImplementationRegistrationOptions"]]
    """ The server provides Goto Implementation support. """
    referencesProvider: NotRequired[Union[bool, "ReferenceOptions"]]
    """ The server provides find references support. """
    documentHighlightProvider: NotRequired[Union[bool, "DocumentHighlightOptions"]]
    """ The server provides document highlight support. """
    documentSymbolProvider: NotRequired[Union[bool, "DocumentSymbolOptions"]]
    """ The server provides document symbol support. """
    codeActionProvider: NotRequired[Union[bool, "CodeActionOptions"]]
    """ The server provides code actions. CodeActionOptions may only be
    specified if the client states that it supports
    `codeActionLiteralSupport` in its initial `initialize` request. """
    codeLensProvider: NotRequired["CodeLensOptions"]
    """ The server provides code lens. """
    documentLinkProvider: NotRequired["DocumentLinkOptions"]
    """ The server provides document link support. """
    colorProvider: NotRequired[Union[bool, "DocumentColorOptions", "DocumentColorRegistrationOptions"]]
    """ The server provides color provider support. """
    workspaceSymbolProvider: NotRequired[Union[bool, "WorkspaceSymbolOptions"]]
    """ The server provides workspace symbol support. """
    documentFormattingProvider: NotRequired[Union[bool, "DocumentFormattingOptions"]]
    """ The server provides document formatting. """
    documentRangeFormattingProvider: NotRequired[Union[bool, "DocumentRangeFormattingOptions"]]
    """ The server provides document range formatting. """
    documentOnTypeFormattingProvider: NotRequired["DocumentOnTypeFormattingOptions"]
    """ The server provides document formatting on typing. """
    renameProvider: NotRequired[Union[bool, "RenameOptions"]]
    """ The server provides rename support. RenameOptions may only be
    specified if the client states that it supports
    `prepareSupport` in its initial `initialize` request. """
    foldingRangeProvider: NotRequired[Union[bool, "FoldingRangeOptions", "FoldingRangeRegistrationOptions"]]
    """ The server provides folding provider support. """
    selectionRangeProvider: NotRequired[Union[bool, "SelectionRangeOptions", "SelectionRangeRegistrationOptions"]]
    """ The server provides selection range support. """
    executeCommandProvider: NotRequired["ExecuteCommandOptions"]
    """ The server provides execute command support. """
    callHierarchyProvider: NotRequired[Union[bool, "CallHierarchyOptions", "CallHierarchyRegistrationOptions"]]
    """ The server provides call hierarchy support.

    @since 3.16.0 """
    linkedEditingRangeProvider: NotRequired[
        Union[bool, "LinkedEditingRangeOptions", "LinkedEditingRangeRegistrationOptions"]
    ]
    """ The server provides linked editing range support.

    @since 3.16.0 """
    semanticTokensProvider: NotRequired[Union["SemanticTokensOptions", "SemanticTokensRegistrationOptions"]]
    """ The server provides semantic tokens support.

    @since 3.16.0 """
    monikerProvider: NotRequired[Union[bool, "MonikerOptions", "MonikerRegistrationOptions"]]
    """ The server provides moniker support.

    @since 3.16.0 """
    typeHierarchyProvider: NotRequired[Union[bool, "TypeHierarchyOptions", "TypeHierarchyRegistrationOptions"]]
    """ The server provides type hierarchy support.

    @since 3.17.0 """
    inlineValueProvider: NotRequired[Union[bool, "InlineValueOptions", "InlineValueRegistrationOptions"]]
    """ The server provides inline values.

    @since 3.17.0 """
    inlayHintProvider: NotRequired[Union[bool, "InlayHintOptions", "InlayHintRegistrationOptions"]]
    """ The server provides inlay hints.

    @since 3.17.0 """
    diagnosticProvider: NotRequired[Union["DiagnosticOptions", "DiagnosticRegistrationOptions"]]
    """ The server has support for pull model diagnostics.

    @since 3.17.0 """
    workspace: NotRequired["__ServerCapabilities_workspace_Type_1"]
    """ Workspace specific server capabilities. """
    experimental: NotRequired["LSPAny"]
    """ Experimental server capabilities. """


class VersionedTextDocumentIdentifier(TypedDict):
    """A text document identifier to denote a specific version of a text document."""

    version: int
    """ The version number of this document. """
    uri: "DocumentUri"
    """ The text document's uri. """


class SaveOptions(TypedDict):
    """Save options."""

    includeText: NotRequired[bool]
    """ The client is supposed to include the content on save. """


class FileEvent(TypedDict):
    """An event describing a file change."""

    uri: "DocumentUri"
    """ The file's uri. """
    type: "FileChangeType"
    """ The change type. """


class FileSystemWatcher(TypedDict):
    globPattern: "GlobPattern"
    """ The glob pattern to watch. See {@link GlobPattern glob pattern} for more detail.

    @since 3.17.0 support for relative patterns. """
    kind: NotRequired["WatchKind"]
    """ The kind of events of interest. If omitted it defaults
    to WatchKind.Create | WatchKind.Change | WatchKind.Delete
    which is 7. """


class Diagnostic(TypedDict):
    """Represents a diagnostic, such as a compiler error or warning. Diagnostic objects
    are only valid in the scope of a resource."""

    range: "Range"
    """ The range at which the message applies """
    severity: NotRequired["DiagnosticSeverity"]
    """ The diagnostic's severity. Can be omitted. If omitted it is up to the
    client to interpret diagnostics as error, warning, info or hint. """
    code: NotRequired[Union[int, str]]
    """ The diagnostic's code, which usually appear in the user interface. """
    codeDescription: NotRequired["CodeDescription"]
    """ An optional property to describe the error code.
    Requires the code field (above) to be present/not null.

    @since 3.16.0 """
    source: NotRequired[str]
    """ A human-readable string describing the source of this
    diagnostic, e.g. 'typescript' or 'super lint'. It usually
    appears in the user interface. """
    message: str
    """ The diagnostic's message. It usually appears in the user interface """
    tags: NotRequired[List["DiagnosticTag"]]
    """ Additional metadata about the diagnostic.

    @since 3.15.0 """
    relatedInformation: NotRequired[List["DiagnosticRelatedInformation"]]
    """ An array of related diagnostic information, e.g. when symbol-names within
    a scope collide all definitions can be marked via this property. """
    data: NotRequired["LSPAny"]
    """ A data entry field that is preserved between a `textDocument/publishDiagnostics`
    notification and `textDocument/codeAction` request.

    @since 3.16.0 """


class CompletionContext(TypedDict):
    """Contains additional information about the context in which a completion request is triggered."""

    triggerKind: "CompletionTriggerKind"
    """ How the completion was triggered. """
    triggerCharacter: NotRequired[str]
    """ The trigger character (a single character) that has trigger code complete.
    Is undefined if `triggerKind !== CompletionTriggerKind.TriggerCharacter` """


class CompletionItemLabelDetails(TypedDict):
    """Additional details for a completion item label.

    @since 3.17.0"""

    detail: NotRequired[str]
    """ An optional string which is rendered less prominently directly after {@link CompletionItem.label label},
    without any spacing. Should be used for function signatures and type annotations. """
    description: NotRequired[str]
    """ An optional string which is rendered less prominently after {@link CompletionItem.detail}. Should be used
    for fully qualified names and file paths. """


class InsertReplaceEdit(TypedDict):
    """A special text edit to provide an insert and a replace operation.

    @since 3.16.0"""

    newText: str
    """ The string to be inserted. """
    insert: "Range"
    """ The range if the insert is requested """
    replace: "Range"
    """ The range if the replace is requested. """


class CompletionOptions(TypedDict):
    """Completion options."""

    triggerCharacters: NotRequired[List[str]]
    """ Most tools trigger completion request automatically without explicitly requesting
    it using a keyboard shortcut (e.g. Ctrl+Space). Typically they do so when the user
    starts to type an identifier. For example if the user types `c` in a JavaScript file
    code complete will automatically pop up present `console` besides others as a
    completion item. Characters that make up identifiers don't need to be listed here.

    If code complete should automatically be trigger on characters not being valid inside
    an identifier (for example `.` in JavaScript) list them in `triggerCharacters`. """
    allCommitCharacters: NotRequired[List[str]]
    """ The list of all possible characters that commit a completion. This field can be used
    if clients don't support individual commit characters per completion item. See
    `ClientCapabilities.textDocument.completion.completionItem.commitCharactersSupport`

    If a server provides both `allCommitCharacters` and commit characters on an individual
    completion item the ones on the completion item win.

    @since 3.2.0 """
    resolveProvider: NotRequired[bool]
    """ The server provides support to resolve additional
    information for a completion item. """
    completionItem: NotRequired["__CompletionOptions_completionItem_Type_2"]
    """ The server supports the following `CompletionItem` specific
    capabilities.

    @since 3.17.0 """
    workDoneProgress: NotRequired[bool]


class HoverOptions(TypedDict):
    """Hover options."""

    workDoneProgress: NotRequired[bool]


class SignatureHelpContext(TypedDict):
    """Additional information about the context in which a signature help request was triggered.

    @since 3.15.0"""

    triggerKind: "SignatureHelpTriggerKind"
    """ Action that caused signature help to be triggered. """
    triggerCharacter: NotRequired[str]
    """ Character that caused signature help to be triggered.

    This is undefined when `triggerKind !== SignatureHelpTriggerKind.TriggerCharacter` """
    isRetrigger: bool
    """ `true` if signature help was already showing when it was triggered.

    Retriggers occurs when the signature help is already active and can be caused by actions such as
    typing a trigger character, a cursor move, or document content changes. """
    activeSignatureHelp: NotRequired["SignatureHelp"]
    """ The currently active `SignatureHelp`.

    The `activeSignatureHelp` has its `SignatureHelp.activeSignature` field updated based on
    the user navigating through available signatures. """


class SignatureInformation(TypedDict):
    """Represents the signature of something callable. A signature
    can have a label, like a function-name, a doc-comment, and
    a set of parameters."""

    label: str
    """ The label of this signature. Will be shown in
    the UI. """
    documentation: NotRequired[Union[str, "MarkupContent"]]
    """ The human-readable doc-comment of this signature. Will be shown
    in the UI but can be omitted. """
    parameters: NotRequired[List["ParameterInformation"]]
    """ The parameters of this signature. """
    activeParameter: NotRequired[Uint]
    """ The index of the active parameter.

    If provided, this is used in place of `SignatureHelp.activeParameter`.

    @since 3.16.0 """


class SignatureHelpOptions(TypedDict):
    """Server Capabilities for a {@link SignatureHelpRequest}."""

    triggerCharacters: NotRequired[List[str]]
    """ List of characters that trigger signature help automatically. """
    retriggerCharacters: NotRequired[List[str]]
    """ List of characters that re-trigger signature help.

    These trigger characters are only active when signature help is already showing. All trigger characters
    are also counted as re-trigger characters.

    @since 3.15.0 """
    workDoneProgress: NotRequired[bool]


class DefinitionOptions(TypedDict):
    """Server Capabilities for a {@link DefinitionRequest}."""

    workDoneProgress: NotRequired[bool]


class ReferenceContext(TypedDict):
    """Value-object that contains additional information when
    requesting references."""

    includeDeclaration: bool
    """ Include the declaration of the current symbol. """


class ReferenceOptions(TypedDict):
    """Reference options."""

    workDoneProgress: NotRequired[bool]


class DocumentHighlightOptions(TypedDict):
    """Provider options for a {@link DocumentHighlightRequest}."""

    workDoneProgress: NotRequired[bool]


class BaseSymbolInformation(TypedDict):
    """A base for all symbol information."""

    name: str
    """ The name of this symbol. """
    kind: "SymbolKind"
    """ The kind of this symbol. """
    tags: NotRequired[List["SymbolTag"]]
    """ Tags for this symbol.

    @since 3.16.0 """
    containerName: NotRequired[str]
    """ The name of the symbol containing this symbol. This information is for
    user interface purposes (e.g. to render a qualifier in the user interface
    if necessary). It can't be used to re-infer a hierarchy for the document
    symbols. """


class DocumentSymbolOptions(TypedDict):
    """Provider options for a {@link DocumentSymbolRequest}."""

    label: NotRequired[str]
    """ A human-readable string that is shown when multiple outlines trees
    are shown for the same document.

    @since 3.16.0 """
    workDoneProgress: NotRequired[bool]


class CodeActionContext(TypedDict):
    """Contains additional diagnostic information about the context in which
    a {@link CodeActionProvider.provideCodeActions code action} is run."""

    diagnostics: List["Diagnostic"]
    """ An array of diagnostics known on the client side overlapping the range provided to the
    `textDocument/codeAction` request. They are provided so that the server knows which
    errors are currently presented to the user for the given range. There is no guarantee
    that these accurately reflect the error state of the resource. The primary parameter
    to compute code actions is the provided range. """
    only: NotRequired[List["CodeActionKind"]]
    """ Requested kind of actions to return.

    Actions not of this kind are filtered out by the client before being shown. So servers
    can omit computing them. """
    triggerKind: NotRequired["CodeActionTriggerKind"]
    """ The reason why code actions were requested.

    @since 3.17.0 """


class CodeActionOptions(TypedDict):
    """Provider options for a {@link CodeActionRequest}."""

    codeActionKinds: NotRequired[List["CodeActionKind"]]
    """ CodeActionKinds that this server may return.

    The list of kinds may be generic, such as `CodeActionKind.Refactor`, or the server
    may list out every specific kind they provide. """
    resolveProvider: NotRequired[bool]
    """ The server provides support to resolve additional
    information for a code action.

    @since 3.16.0 """
    workDoneProgress: NotRequired[bool]


class WorkspaceSymbolOptions(TypedDict):
    """Server capabilities for a {@link WorkspaceSymbolRequest}."""

    resolveProvider: NotRequired[bool]
    """ The server provides support to resolve additional
    information for a workspace symbol.

    @since 3.17.0 """
    workDoneProgress: NotRequired[bool]


class CodeLensOptions(TypedDict):
    """Code Lens provider options of a {@link CodeLensRequest}."""

    resolveProvider: NotRequired[bool]
    """ Code lens has a resolve provider as well. """
    workDoneProgress: NotRequired[bool]


class DocumentLinkOptions(TypedDict):
    """Provider options for a {@link DocumentLinkRequest}."""

    resolveProvider: NotRequired[bool]
    """ Document links have a resolve provider as well. """
    workDoneProgress: NotRequired[bool]


class FormattingOptions(TypedDict):
    """Value-object describing what options formatting should use."""

    tabSize: Uint
    """ Size of a tab in spaces. """
    insertSpaces: bool
    """ Prefer spaces over tabs. """
    trimTrailingWhitespace: NotRequired[bool]
    """ Trim trailing whitespace on a line.

    @since 3.15.0 """
    insertFinalNewline: NotRequired[bool]
    """ Insert a newline character at the end of the file if one does not exist.

    @since 3.15.0 """
    trimFinalNewlines: NotRequired[bool]
    """ Trim all newlines after the final newline at the end of the file.

    @since 3.15.0 """


class DocumentFormattingOptions(TypedDict):
    """Provider options for a {@link DocumentFormattingRequest}."""

    workDoneProgress: NotRequired[bool]


class DocumentRangeFormattingOptions(TypedDict):
    """Provider options for a {@link DocumentRangeFormattingRequest}."""

    workDoneProgress: NotRequired[bool]


class DocumentOnTypeFormattingOptions(TypedDict):
    """Provider options for a {@link DocumentOnTypeFormattingRequest}."""

    firstTriggerCharacter: str
    """ A character on which formatting should be triggered, like `{`. """
    moreTriggerCharacter: NotRequired[List[str]]
    """ More trigger characters. """


class RenameOptions(TypedDict):
    """Provider options for a {@link RenameRequest}."""

    prepareProvider: NotRequired[bool]
    """ Renames should be checked and tested before being executed.

    @since version 3.12.0 """
    workDoneProgress: NotRequired[bool]


class ExecuteCommandOptions(TypedDict):
    """The server capabilities of a {@link ExecuteCommandRequest}."""

    commands: List[str]
    """ The commands to be executed on the server """
    workDoneProgress: NotRequired[bool]


class SemanticTokensLegend(TypedDict):
    """@since 3.16.0"""

    tokenTypes: List[str]
    """ The token types a server uses. """
    tokenModifiers: List[str]
    """ The token modifiers a server uses. """


class OptionalVersionedTextDocumentIdentifier(TypedDict):
    """A text document identifier to optionally denote a specific version of a text document."""

    version: Union[int, None]
    """ The version number of this document. If a versioned text document identifier
    is sent from the server to the client and the file is not open in the editor
    (the server has not received an open notification before) the server can send
    `null` to indicate that the version is unknown and the content on disk is the
    truth (as specified with document content ownership). """
    uri: "DocumentUri"
    """ The text document's uri. """


class AnnotatedTextEdit(TypedDict):
    """A special text edit with an additional change annotation.

    @since 3.16.0."""

    annotationId: "ChangeAnnotationIdentifier"
    """ The actual identifier of the change annotation """
    range: "Range"
    """ The range of the text document to be manipulated. To insert
    text into a document create a range where start === end. """
    newText: str
    """ The string to be inserted. For delete operations use an
    empty string. """


class ResourceOperation(TypedDict):
    """A generic resource operation."""

    kind: str
    """ The resource operation kind. """
    annotationId: NotRequired["ChangeAnnotationIdentifier"]
    """ An optional annotation identifier describing the operation.

    @since 3.16.0 """


class CreateFileOptions(TypedDict):
    """Options to create a file."""

    overwrite: NotRequired[bool]
    """ Overwrite existing file. Overwrite wins over `ignoreIfExists` """
    ignoreIfExists: NotRequired[bool]
    """ Ignore if exists. """


class RenameFileOptions(TypedDict):
    """Rename file options"""

    overwrite: NotRequired[bool]
    """ Overwrite target if existing. Overwrite wins over `ignoreIfExists` """
    ignoreIfExists: NotRequired[bool]
    """ Ignores if target exists. """


class DeleteFileOptions(TypedDict):
    """Delete file options"""

    recursive: NotRequired[bool]
    """ Delete the content recursively if a folder is denoted. """
    ignoreIfNotExists: NotRequired[bool]
    """ Ignore the operation if the file doesn't exist. """


class FileOperationPattern(TypedDict):
    """A pattern to describe in which file operation requests or notifications
    the server is interested in receiving.

    @since 3.16.0"""

    glob: str
    """ The glob pattern to match. Glob patterns can have the following syntax:
    - `*` to match one or more characters in a path segment
    - `?` to match on one character in a path segment
    - `**` to match any number of path segments, including none
    - `{}` to group sub patterns into an OR expression. (e.g. `**​/*.{ts,js}` matches all TypeScript and JavaScript files)
    - `[]` to declare a range of characters to match in a path segment (e.g., `example.[0-9]` to match on `example.0`, `example.1`, …)
    - `[!...]` to negate a range of characters to match in a path segment (e.g., `example.[!0-9]` to match on `example.a`, `example.b`, but not `example.0`) """
    matches: NotRequired["FileOperationPatternKind"]
    """ Whether to match files or folders with this pattern.

    Matches both if undefined. """
    options: NotRequired["FileOperationPatternOptions"]
    """ Additional options used during matching. """


class WorkspaceFullDocumentDiagnosticReport(TypedDict):
    """A full document diagnostic report for a workspace diagnostic result.

    @since 3.17.0"""

    uri: "DocumentUri"
    """ The URI for which diagnostic information is reported. """
    version: Union[int, None]
    """ The version number for which the diagnostics are reported.
    If the document is not marked as open `null` can be provided. """
    kind: Literal["full"]
    """ A full document diagnostic report. """
    resultId: NotRequired[str]
    """ An optional result id. If provided it will
    be sent on the next diagnostic request for the
    same document. """
    items: List["Diagnostic"]
    """ The actual items. """


class WorkspaceUnchangedDocumentDiagnosticReport(TypedDict):
    """An unchanged document diagnostic report for a workspace diagnostic result.

    @since 3.17.0"""

    uri: "DocumentUri"
    """ The URI for which diagnostic information is reported. """
    version: Union[int, None]
    """ The version number for which the diagnostics are reported.
    If the document is not marked as open `null` can be provided. """
    kind: Literal["unchanged"]
    """ A document diagnostic report indicating
    no changes to the last result. A server can
    only return `unchanged` if result ids are
    provided. """
    resultId: str
    """ A result id which will be sent on the next
    diagnostic request for the same document. """


class NotebookCell(TypedDict):
    """A notebook cell.

    A cell's document URI must be unique across ALL notebook
    cells and can therefore be used to uniquely identify a
    notebook cell or the cell's text document.

    @since 3.17.0"""

    kind: "NotebookCellKind"
    """ The cell's kind """
    document: "DocumentUri"
    """ The URI of the cell's text document
    content. """
    metadata: NotRequired["LSPObject"]
    """ Additional metadata stored with the cell.

    Note: should always be an object literal (e.g. LSPObject) """
    executionSummary: NotRequired["ExecutionSummary"]
    """ Additional execution summary information
    if supported by the client. """


class NotebookCellArrayChange(TypedDict):
    """A change describing how to move a `NotebookCell`
    array from state S to S'.

    @since 3.17.0"""

    start: Uint
    """ The start oftest of the cell that changed. """
    deleteCount: Uint
    """ The deleted cells """
    cells: NotRequired[List["NotebookCell"]]
    """ The new cells, if any """


class ClientCapabilities(TypedDict):
    """Defines the capabilities provided by the client."""

    workspace: NotRequired["WorkspaceClientCapabilities"]
    """ Workspace specific client capabilities. """
    textDocument: NotRequired["TextDocumentClientCapabilities"]
    """ Text document specific client capabilities. """
    notebookDocument: NotRequired["NotebookDocumentClientCapabilities"]
    """ Capabilities specific to the notebook document support.

    @since 3.17.0 """
    window: NotRequired["WindowClientCapabilities"]
    """ Window specific client capabilities. """
    general: NotRequired["GeneralClientCapabilities"]
    """ General client capabilities.

    @since 3.16.0 """
    experimental: NotRequired["LSPAny"]
    """ Experimental client capabilities. """


class TextDocumentSyncOptions(TypedDict):
    openClose: NotRequired[bool]
    """ Open and close notifications are sent to the server. If omitted open close notification should not
    be sent. """
    change: NotRequired["TextDocumentSyncKind"]
    """ Change notifications are sent to the server. See TextDocumentSyncKind.None, TextDocumentSyncKind.Full
    and TextDocumentSyncKind.Incremental. If omitted it defaults to TextDocumentSyncKind.None. """
    willSave: NotRequired[bool]
    """ If present will save notifications are sent to the server. If omitted the notification should not be
    sent. """
    willSaveWaitUntil: NotRequired[bool]
    """ If present will save wait until requests are sent to the server. If omitted the request should not be
    sent. """
    save: NotRequired[Union[bool, "SaveOptions"]]
    """ If present save notifications are sent to the server. If omitted the notification should not be
    sent. """


class NotebookDocumentSyncOptions(TypedDict):
    """Options specific to a notebook plus its cells
    to be synced to the server.

    If a selector provides a notebook document
    filter but no cell selector all cells of a
    matching notebook document will be synced.

    If a selector provides no notebook document
    filter but only a cell selector all notebook
    document that contain at least one matching
    cell will be synced.

    @since 3.17.0"""

    notebookSelector: List[
        Union[
            "__NotebookDocumentSyncOptions_notebookSelector_Type_1",
            "__NotebookDocumentSyncOptions_notebookSelector_Type_2",
        ]
    ]
    """ The notebooks to be synced """
    save: NotRequired[bool]
    """ Whether save notification should be forwarded to
    the server. Will only be honored if mode === `notebook`. """


class NotebookDocumentSyncRegistrationOptions(TypedDict):
    """Registration options specific to a notebook.

    @since 3.17.0"""

    notebookSelector: List[
        Union[
            "__NotebookDocumentSyncOptions_notebookSelector_Type_3",
            "__NotebookDocumentSyncOptions_notebookSelector_Type_4",
        ]
    ]
    """ The notebooks to be synced """
    save: NotRequired[bool]
    """ Whether save notification should be forwarded to
    the server. Will only be honored if mode === `notebook`. """
    id: NotRequired[str]
    """ The id used to register the request. The id can be used to deregister
    the request again. See also Registration#id. """


class WorkspaceFoldersServerCapabilities(TypedDict):
    supported: NotRequired[bool]
    """ The server has support for workspace folders """
    changeNotifications: NotRequired[Union[str, bool]]
    """ Whether the server wants to receive workspace folder
    change notifications.

    If a string is provided the string is treated as an ID
    under which the notification is registered on the client
    side. The ID can be used to unregister for these events
    using the `client/unregisterCapability` request. """


class FileOperationOptions(TypedDict):
    """Options for notifications/requests for user operations on files.

    @since 3.16.0"""

    didCreate: NotRequired["FileOperationRegistrationOptions"]
    """ The server is interested in receiving didCreateFiles notifications. """
    willCreate: NotRequired["FileOperationRegistrationOptions"]
    """ The server is interested in receiving willCreateFiles requests. """
    didRename: NotRequired["FileOperationRegistrationOptions"]
    """ The server is interested in receiving didRenameFiles notifications. """
    willRename: NotRequired["FileOperationRegistrationOptions"]
    """ The server is interested in receiving willRenameFiles requests. """
    didDelete: NotRequired["FileOperationRegistrationOptions"]
    """ The server is interested in receiving didDeleteFiles file notifications. """
    willDelete: NotRequired["FileOperationRegistrationOptions"]
    """ The server is interested in receiving willDeleteFiles file requests. """


class CodeDescription(TypedDict):
    """Structure to capture a description for an error code.

    @since 3.16.0"""

    href: "URI"
    """ An URI to open with more information about the diagnostic error. """


class DiagnosticRelatedInformation(TypedDict):
    """Represents a related message and source code location for a diagnostic. This should be
    used to point to code locations that cause or related to a diagnostics, e.g when duplicating
    a symbol in a scope."""

    location: "Location"
    """ The location of this related diagnostic information. """
    message: str
    """ The message of this related diagnostic information. """


class ParameterInformation(TypedDict):
    """Represents a parameter of a callable-signature. A parameter can
    have a label and a doc-comment."""

    label: Union[str, List[Union[Uint, Uint]]]
    """ The label of this parameter information.

    Either a string or an inclusive start and exclusive end offsets within its containing
    signature label. (see SignatureInformation.label). The offsets are based on a UTF-16
    string representation as `Position` and `Range` does.

    *Note*: a label of type string should be a substring of its containing signature label.
    Its intended use case is to highlight the parameter label part in the `SignatureInformation.label`. """
    documentation: NotRequired[Union[str, "MarkupContent"]]
    """ The human-readable doc-comment of this parameter. Will be shown
    in the UI but can be omitted. """


class NotebookCellTextDocumentFilter(TypedDict):
    """A notebook cell text document filter denotes a cell text
    document by different properties.

    @since 3.17.0"""

    notebook: Union[str, "NotebookDocumentFilter"]
    """ A filter that matches against the notebook
    containing the notebook cell. If a string
    value is provided it matches against the
    notebook type. '*' matches every notebook. """
    language: NotRequired[str]
    """ A language id like `python`.

    Will be matched against the language id of the
    notebook cell document. '*' matches every language. """


class FileOperationPatternOptions(TypedDict):
    """Matching options for the file operation pattern.

    @since 3.16.0"""

    ignoreCase: NotRequired[bool]
    """ The pattern should be matched ignoring casing. """


class ExecutionSummary(TypedDict):
    executionOrder: Uint
    """ A strict monotonically increasing value
    indicating the execution order of a cell
    inside a notebook. """
    success: NotRequired[bool]
    """ Whether the execution was successful or
    not if known by the client. """


class WorkspaceClientCapabilities(TypedDict):
    """Workspace specific client capabilities."""

    applyEdit: NotRequired[bool]
    """ The client supports applying batch edits
    to the workspace by supporting the request
    'workspace/applyEdit' """
    workspaceEdit: NotRequired["WorkspaceEditClientCapabilities"]
    """ Capabilities specific to `WorkspaceEdit`s. """
    didChangeConfiguration: NotRequired["DidChangeConfigurationClientCapabilities"]
    """ Capabilities specific to the `workspace/didChangeConfiguration` notification. """
    didChangeWatchedFiles: NotRequired["DidChangeWatchedFilesClientCapabilities"]
    """ Capabilities specific to the `workspace/didChangeWatchedFiles` notification. """
    symbol: NotRequired["WorkspaceSymbolClientCapabilities"]
    """ Capabilities specific to the `workspace/symbol` request. """
    executeCommand: NotRequired["ExecuteCommandClientCapabilities"]
    """ Capabilities specific to the `workspace/executeCommand` request. """
    workspaceFolders: NotRequired[bool]
    """ The client has support for workspace folders.

    @since 3.6.0 """
    configuration: NotRequired[bool]
    """ The client supports `workspace/configuration` requests.

    @since 3.6.0 """
    semanticTokens: NotRequired["SemanticTokensWorkspaceClientCapabilities"]
    """ Capabilities specific to the semantic token requests scoped to the
    workspace.

    @since 3.16.0. """
    codeLens: NotRequired["CodeLensWorkspaceClientCapabilities"]
    """ Capabilities specific to the code lens requests scoped to the
    workspace.

    @since 3.16.0. """
    fileOperations: NotRequired["FileOperationClientCapabilities"]
    """ The client has support for file notifications/requests for user operations on files.

    Since 3.16.0 """
    inlineValue: NotRequired["InlineValueWorkspaceClientCapabilities"]
    """ Capabilities specific to the inline values requests scoped to the
    workspace.

    @since 3.17.0. """
    inlayHint: NotRequired["InlayHintWorkspaceClientCapabilities"]
    """ Capabilities specific to the inlay hint requests scoped to the
    workspace.

    @since 3.17.0. """
    diagnostics: NotRequired["DiagnosticWorkspaceClientCapabilities"]
    """ Capabilities specific to the diagnostic requests scoped to the
    workspace.

    @since 3.17.0. """


class TextDocumentClientCapabilities(TypedDict):
    """Text document specific client capabilities."""

    synchronization: NotRequired["TextDocumentSyncClientCapabilities"]
    """ Defines which synchronization capabilities the client supports. """
    completion: NotRequired["CompletionClientCapabilities"]
    """ Capabilities specific to the `textDocument/completion` request. """
    hover: NotRequired["HoverClientCapabilities"]
    """ Capabilities specific to the `textDocument/hover` request. """
    signatureHelp: NotRequired["SignatureHelpClientCapabilities"]
    """ Capabilities specific to the `textDocument/signatureHelp` request. """
    declaration: NotRequired["DeclarationClientCapabilities"]
    """ Capabilities specific to the `textDocument/declaration` request.

    @since 3.14.0 """
    definition: NotRequired["DefinitionClientCapabilities"]
    """ Capabilities specific to the `textDocument/definition` request. """
    typeDefinition: NotRequired["TypeDefinitionClientCapabilities"]
    """ Capabilities specific to the `textDocument/typeDefinition` request.

    @since 3.6.0 """
    implementation: NotRequired["ImplementationClientCapabilities"]
    """ Capabilities specific to the `textDocument/implementation` request.

    @since 3.6.0 """
    references: NotRequired["ReferenceClientCapabilities"]
    """ Capabilities specific to the `textDocument/references` request. """
    documentHighlight: NotRequired["DocumentHighlightClientCapabilities"]
    """ Capabilities specific to the `textDocument/documentHighlight` request. """
    documentSymbol: NotRequired["DocumentSymbolClientCapabilities"]
    """ Capabilities specific to the `textDocument/documentSymbol` request. """
    codeAction: NotRequired["CodeActionClientCapabilities"]
    """ Capabilities specific to the `textDocument/codeAction` request. """
    codeLens: NotRequired["CodeLensClientCapabilities"]
    """ Capabilities specific to the `textDocument/codeLens` request. """
    documentLink: NotRequired["DocumentLinkClientCapabilities"]
    """ Capabilities specific to the `textDocument/documentLink` request. """
    colorProvider: NotRequired["DocumentColorClientCapabilities"]
    """ Capabilities specific to the `textDocument/documentColor` and the
    `textDocument/colorPresentation` request.

    @since 3.6.0 """
    formatting: NotRequired["DocumentFormattingClientCapabilities"]
    """ Capabilities specific to the `textDocument/formatting` request. """
    rangeFormatting: NotRequired["DocumentRangeFormattingClientCapabilities"]
    """ Capabilities specific to the `textDocument/rangeFormatting` request. """
    onTypeFormatting: NotRequired["DocumentOnTypeFormattingClientCapabilities"]
    """ Capabilities specific to the `textDocument/onTypeFormatting` request. """
    rename: NotRequired["RenameClientCapabilities"]
    """ Capabilities specific to the `textDocument/rename` request. """
    foldingRange: NotRequired["FoldingRangeClientCapabilities"]
    """ Capabilities specific to the `textDocument/foldingRange` request.

    @since 3.10.0 """
    selectionRange: NotRequired["SelectionRangeClientCapabilities"]
    """ Capabilities specific to the `textDocument/selectionRange` request.

    @since 3.15.0 """
    publishDiagnostics: NotRequired["PublishDiagnosticsClientCapabilities"]
    """ Capabilities specific to the `textDocument/publishDiagnostics` notification. """
    callHierarchy: NotRequired["CallHierarchyClientCapabilities"]
    """ Capabilities specific to the various call hierarchy requests.

    @since 3.16.0 """
    semanticTokens: NotRequired["SemanticTokensClientCapabilities"]
    """ Capabilities specific to the various semantic token request.

    @since 3.16.0 """
    linkedEditingRange: NotRequired["LinkedEditingRangeClientCapabilities"]
    """ Capabilities specific to the `textDocument/linkedEditingRange` request.

    @since 3.16.0 """
    moniker: NotRequired["MonikerClientCapabilities"]
    """ Client capabilities specific to the `textDocument/moniker` request.

    @since 3.16.0 """
    typeHierarchy: NotRequired["TypeHierarchyClientCapabilities"]
    """ Capabilities specific to the various type hierarchy requests.

    @since 3.17.0 """
    inlineValue: NotRequired["InlineValueClientCapabilities"]
    """ Capabilities specific to the `textDocument/inlineValue` request.

    @since 3.17.0 """
    inlayHint: NotRequired["InlayHintClientCapabilities"]
    """ Capabilities specific to the `textDocument/inlayHint` request.

    @since 3.17.0 """
    diagnostic: NotRequired["DiagnosticClientCapabilities"]
    """ Capabilities specific to the diagnostic pull model.

    @since 3.17.0 """


class NotebookDocumentClientCapabilities(TypedDict):
    """Capabilities specific to the notebook document support.

    @since 3.17.0"""

    synchronization: "NotebookDocumentSyncClientCapabilities"
    """ Capabilities specific to notebook document synchronization

    @since 3.17.0 """


class WindowClientCapabilities(TypedDict):
    workDoneProgress: NotRequired[bool]
    """ It indicates whether the client supports server initiated
    progress using the `window/workDoneProgress/create` request.

    The capability also controls Whether client supports handling
    of progress notifications. If set servers are allowed to report a
    `workDoneProgress` property in the request specific server
    capabilities.

    @since 3.15.0 """
    showMessage: NotRequired["ShowMessageRequestClientCapabilities"]
    """ Capabilities specific to the showMessage request.

    @since 3.16.0 """
    showDocument: NotRequired["ShowDocumentClientCapabilities"]
    """ Capabilities specific to the showDocument request.

    @since 3.16.0 """


class GeneralClientCapabilities(TypedDict):
    """General client capabilities.

    @since 3.16.0"""

    staleRequestSupport: NotRequired["__GeneralClientCapabilities_staleRequestSupport_Type_1"]
    """ Client capability that signals how the client
    handles stale requests (e.g. a request
    for which the client will not process the response
    anymore since the information is outdated).

    @since 3.17.0 """
    regularExpressions: NotRequired["RegularExpressionsClientCapabilities"]
    """ Client capabilities specific to regular expressions.

    @since 3.16.0 """
    markdown: NotRequired["MarkdownClientCapabilities"]
    """ Client capabilities specific to the client's markdown parser.

    @since 3.16.0 """
    positionEncodings: NotRequired[List["PositionEncodingKind"]]
    """ The position encodings supported by the client. Client and server
    have to agree on the same position encoding to ensure that offsets
    (e.g. character position in a line) are interpreted the same on both
    sides.

    To keep the protocol backwards compatible the following applies: if
    the value 'utf-16' is missing from the array of position encodings
    servers can assume that the client supports UTF-16. UTF-16 is
    therefore a mandatory encoding.

    If omitted it defaults to ['utf-16'].

    Implementation considerations: since the conversion from one encoding
    into another requires the content of the file / line the conversion
    is best done where the file is read which is usually on the server
    side.

    @since 3.17.0 """


class RelativePattern(TypedDict):
    """A relative pattern is a helper to construct glob patterns that are matched
    relatively to a base URI. The common value for a `baseUri` is a workspace
    folder root, but it can be another absolute URI as well.

    @since 3.17.0"""

    baseUri: Union["WorkspaceFolder", "URI"]
    """ A workspace folder or a base URI to which this pattern will be matched
    against relatively. """
    pattern: "Pattern"
    """ The actual glob pattern; """


class WorkspaceEditClientCapabilities(TypedDict):
    documentChanges: NotRequired[bool]
    """ The client supports versioned document changes in `WorkspaceEdit`s """
    resourceOperations: NotRequired[List["ResourceOperationKind"]]
    """ The resource operations the client supports. Clients should at least
    support 'create', 'rename' and 'delete' files and folders.

    @since 3.13.0 """
    failureHandling: NotRequired["FailureHandlingKind"]
    """ The failure handling strategy of a client if applying the workspace edit
    fails.

    @since 3.13.0 """
    normalizesLineEndings: NotRequired[bool]
    """ Whether the client normalizes line endings to the client specific
    setting.
    If set to `true` the client will normalize line ending characters
    in a workspace edit to the client-specified new line
    character.

    @since 3.16.0 """
    changeAnnotationSupport: NotRequired["__WorkspaceEditClientCapabilities_changeAnnotationSupport_Type_1"]
    """ Whether the client in general supports change annotations on text edits,
    create file, rename file and delete file changes.

    @since 3.16.0 """


class DidChangeConfigurationClientCapabilities(TypedDict):
    dynamicRegistration: NotRequired[bool]
    """ Did change configuration notification supports dynamic registration. """


class DidChangeWatchedFilesClientCapabilities(TypedDict):
    dynamicRegistration: NotRequired[bool]
    """ Did change watched files notification supports dynamic registration. Please note
    that the current protocol doesn't support static configuration for file changes
    from the server side. """
    relativePatternSupport: NotRequired[bool]
    """ Whether the client has support for {@link  RelativePattern relative pattern}
    or not.

    @since 3.17.0 """


class WorkspaceSymbolClientCapabilities(TypedDict):
    """Client capabilities for a {@link WorkspaceSymbolRequest}."""

    dynamicRegistration: NotRequired[bool]
    """ Symbol request supports dynamic registration. """
    symbolKind: NotRequired["__WorkspaceSymbolClientCapabilities_symbolKind_Type_1"]
    """ Specific capabilities for the `SymbolKind` in the `workspace/symbol` request. """
    tagSupport: NotRequired["__WorkspaceSymbolClientCapabilities_tagSupport_Type_1"]
    """ The client supports tags on `SymbolInformation`.
    Clients supporting tags have to handle unknown tags gracefully.

    @since 3.16.0 """
    resolveSupport: NotRequired["__WorkspaceSymbolClientCapabilities_resolveSupport_Type_1"]
    """ The client support partial workspace symbols. The client will send the
    request `workspaceSymbol/resolve` to the server to resolve additional
    properties.

    @since 3.17.0 """


class ExecuteCommandClientCapabilities(TypedDict):
    """The client capabilities of a {@link ExecuteCommandRequest}."""

    dynamicRegistration: NotRequired[bool]
    """ Execute command supports dynamic registration. """


class SemanticTokensWorkspaceClientCapabilities(TypedDict):
    """@since 3.16.0"""

    refreshSupport: NotRequired[bool]
    """ Whether the client implementation supports a refresh request sent from
    the server to the client.

    Note that this event is global and will force the client to refresh all
    semantic tokens currently shown. It should be used with absolute care
    and is useful for situation where a server for example detects a project
    wide change that requires such a calculation. """


class CodeLensWorkspaceClientCapabilities(TypedDict):
    """@since 3.16.0"""

    refreshSupport: NotRequired[bool]
    """ Whether the client implementation supports a refresh request sent from the
    server to the client.

    Note that this event is global and will force the client to refresh all
    code lenses currently shown. It should be used with absolute care and is
    useful for situation where a server for example detect a project wide
    change that requires such a calculation. """


class FileOperationClientCapabilities(TypedDict):
    """Capabilities relating to events from file operations by the user in the client.

    These events do not come from the file system, they come from user operations
    like renaming a file in the UI.

    @since 3.16.0"""

    dynamicRegistration: NotRequired[bool]
    """ Whether the client supports dynamic registration for file requests/notifications. """
    didCreate: NotRequired[bool]
    """ The client has support for sending didCreateFiles notifications. """
    willCreate: NotRequired[bool]
    """ The client has support for sending willCreateFiles requests. """
    didRename: NotRequired[bool]
    """ The client has support for sending didRenameFiles notifications. """
    willRename: NotRequired[bool]
    """ The client has support for sending willRenameFiles requests. """
    didDelete: NotRequired[bool]
    """ The client has support for sending didDeleteFiles notifications. """
    willDelete: NotRequired[bool]
    """ The client has support for sending willDeleteFiles requests. """


class InlineValueWorkspaceClientCapabilities(TypedDict):
    """Client workspace capabilities specific to inline values.

    @since 3.17.0"""

    refreshSupport: NotRequired[bool]
    """ Whether the client implementation supports a refresh request sent from the
    server to the client.

    Note that this event is global and will force the client to refresh all
    inline values currently shown. It should be used with absolute care and is
    useful for situation where a server for example detects a project wide
    change that requires such a calculation. """


class InlayHintWorkspaceClientCapabilities(TypedDict):
    """Client workspace capabilities specific to inlay hints.

    @since 3.17.0"""

    refreshSupport: NotRequired[bool]
    """ Whether the client implementation supports a refresh request sent from
    the server to the client.

    Note that this event is global and will force the client to refresh all
    inlay hints currently shown. It should be used with absolute care and
    is useful for situation where a server for example detects a project wide
    change that requires such a calculation. """


class DiagnosticWorkspaceClientCapabilities(TypedDict):
    """Workspace client capabilities specific to diagnostic pull requests.

    @since 3.17.0"""

    refreshSupport: NotRequired[bool]
    """ Whether the client implementation supports a refresh request sent from
    the server to the client.

    Note that this event is global and will force the client to refresh all
    pulled diagnostics currently shown. It should be used with absolute care and
    is useful for situation where a server for example detects a project wide
    change that requires such a calculation. """


class TextDocumentSyncClientCapabilities(TypedDict):
    dynamicRegistration: NotRequired[bool]
    """ Whether text document synchronization supports dynamic registration. """
    willSave: NotRequired[bool]
    """ The client supports sending will save notifications. """
    willSaveWaitUntil: NotRequired[bool]
    """ The client supports sending a will save request and
    waits for a response providing text edits which will
    be applied to the document before it is saved. """
    didSave: NotRequired[bool]
    """ The client supports did save notifications. """


class CompletionClientCapabilities(TypedDict):
    """Completion client capabilities"""

    dynamicRegistration: NotRequired[bool]
    """ Whether completion supports dynamic registration. """
    completionItem: NotRequired["__CompletionClientCapabilities_completionItem_Type_1"]
    """ The client supports the following `CompletionItem` specific
    capabilities. """
    completionItemKind: NotRequired["__CompletionClientCapabilities_completionItemKind_Type_1"]
    insertTextMode: NotRequired["InsertTextMode"]
    """ Defines how the client handles whitespace and indentation
    when accepting a completion item that uses multi line
    text in either `insertText` or `textEdit`.

    @since 3.17.0 """
    contextSupport: NotRequired[bool]
    """ The client supports to send additional context information for a
    `textDocument/completion` request. """
    completionList: NotRequired["__CompletionClientCapabilities_completionList_Type_1"]
    """ The client supports the following `CompletionList` specific
    capabilities.

    @since 3.17.0 """


class HoverClientCapabilities(TypedDict):
    dynamicRegistration: NotRequired[bool]
    """ Whether hover supports dynamic registration. """
    contentFormat: NotRequired[List["MarkupKind"]]
    """ Client supports the following content formats for the content
    property. The order describes the preferred format of the client. """


class SignatureHelpClientCapabilities(TypedDict):
    """Client Capabilities for a {@link SignatureHelpRequest}."""

    dynamicRegistration: NotRequired[bool]
    """ Whether signature help supports dynamic registration. """
    signatureInformation: NotRequired["__SignatureHelpClientCapabilities_signatureInformation_Type_1"]
    """ The client supports the following `SignatureInformation`
    specific properties. """
    contextSupport: NotRequired[bool]
    """ The client supports to send additional context information for a
    `textDocument/signatureHelp` request. A client that opts into
    contextSupport will also support the `retriggerCharacters` on
    `SignatureHelpOptions`.

    @since 3.15.0 """


class DeclarationClientCapabilities(TypedDict):
    """@since 3.14.0"""

    dynamicRegistration: NotRequired[bool]
    """ Whether declaration supports dynamic registration. If this is set to `true`
    the client supports the new `DeclarationRegistrationOptions` return value
    for the corresponding server capability as well. """
    linkSupport: NotRequired[bool]
    """ The client supports additional metadata in the form of declaration links. """


class DefinitionClientCapabilities(TypedDict):
    """Client Capabilities for a {@link DefinitionRequest}."""

    dynamicRegistration: NotRequired[bool]
    """ Whether definition supports dynamic registration. """
    linkSupport: NotRequired[bool]
    """ The client supports additional metadata in the form of definition links.

    @since 3.14.0 """


class TypeDefinitionClientCapabilities(TypedDict):
    """Since 3.6.0"""

    dynamicRegistration: NotRequired[bool]
    """ Whether implementation supports dynamic registration. If this is set to `true`
    the client supports the new `TypeDefinitionRegistrationOptions` return value
    for the corresponding server capability as well. """
    linkSupport: NotRequired[bool]
    """ The client supports additional metadata in the form of definition links.

    Since 3.14.0 """


class ImplementationClientCapabilities(TypedDict):
    """@since 3.6.0"""

    dynamicRegistration: NotRequired[bool]
    """ Whether implementation supports dynamic registration. If this is set to `true`
    the client supports the new `ImplementationRegistrationOptions` return value
    for the corresponding server capability as well. """
    linkSupport: NotRequired[bool]
    """ The client supports additional metadata in the form of definition links.

    @since 3.14.0 """


class ReferenceClientCapabilities(TypedDict):
    """Client Capabilities for a {@link ReferencesRequest}."""

    dynamicRegistration: NotRequired[bool]
    """ Whether references supports dynamic registration. """


class DocumentHighlightClientCapabilities(TypedDict):
    """Client Capabilities for a {@link DocumentHighlightRequest}."""

    dynamicRegistration: NotRequired[bool]
    """ Whether document highlight supports dynamic registration. """


class DocumentSymbolClientCapabilities(TypedDict):
    """Client Capabilities for a {@link DocumentSymbolRequest}."""

    dynamicRegistration: NotRequired[bool]
    """ Whether document symbol supports dynamic registration. """
    symbolKind: NotRequired["__DocumentSymbolClientCapabilities_symbolKind_Type_1"]
    """ Specific capabilities for the `SymbolKind` in the
    `textDocument/documentSymbol` request. """
    hierarchicalDocumentSymbolSupport: NotRequired[bool]
    """ The client supports hierarchical document symbols. """
    tagSupport: NotRequired["__DocumentSymbolClientCapabilities_tagSupport_Type_1"]
    """ The client supports tags on `SymbolInformation`. Tags are supported on
    `DocumentSymbol` if `hierarchicalDocumentSymbolSupport` is set to true.
    Clients supporting tags have to handle unknown tags gracefully.

    @since 3.16.0 """
    labelSupport: NotRequired[bool]
    """ The client supports an additional label presented in the UI when
    registering a document symbol provider.

    @since 3.16.0 """


class CodeActionClientCapabilities(TypedDict):
    """The Client Capabilities of a {@link CodeActionRequest}."""

    dynamicRegistration: NotRequired[bool]
    """ Whether code action supports dynamic registration. """
    codeActionLiteralSupport: NotRequired["__CodeActionClientCapabilities_codeActionLiteralSupport_Type_1"]
    """ The client support code action literals of type `CodeAction` as a valid
    response of the `textDocument/codeAction` request. If the property is not
    set the request can only return `Command` literals.

    @since 3.8.0 """
    isPreferredSupport: NotRequired[bool]
    """ Whether code action supports the `isPreferred` property.

    @since 3.15.0 """
    disabledSupport: NotRequired[bool]
    """ Whether code action supports the `disabled` property.

    @since 3.16.0 """
    dataSupport: NotRequired[bool]
    """ Whether code action supports the `data` property which is
    preserved between a `textDocument/codeAction` and a
    `codeAction/resolve` request.

    @since 3.16.0 """
    resolveSupport: NotRequired["__CodeActionClientCapabilities_resolveSupport_Type_1"]
    """ Whether the client supports resolving additional code action
    properties via a separate `codeAction/resolve` request.

    @since 3.16.0 """
    honorsChangeAnnotations: NotRequired[bool]
    """ Whether the client honors the change annotations in
    text edits and resource operations returned via the
    `CodeAction#edit` property by for example presenting
    the workspace edit in the user interface and asking
    for confirmation.

    @since 3.16.0 """


class CodeLensClientCapabilities(TypedDict):
    """The client capabilities  of a {@link CodeLensRequest}."""

    dynamicRegistration: NotRequired[bool]
    """ Whether code lens supports dynamic registration. """


class DocumentLinkClientCapabilities(TypedDict):
    """The client capabilities of a {@link DocumentLinkRequest}."""

    dynamicRegistration: NotRequired[bool]
    """ Whether document link supports dynamic registration. """
    tooltipSupport: NotRequired[bool]
    """ Whether the client supports the `tooltip` property on `DocumentLink`.

    @since 3.15.0 """


class DocumentColorClientCapabilities(TypedDict):
    dynamicRegistration: NotRequired[bool]
    """ Whether implementation supports dynamic registration. If this is set to `true`
    the client supports the new `DocumentColorRegistrationOptions` return value
    for the corresponding server capability as well. """


class DocumentFormattingClientCapabilities(TypedDict):
    """Client capabilities of a {@link DocumentFormattingRequest}."""

    dynamicRegistration: NotRequired[bool]
    """ Whether formatting supports dynamic registration. """


class DocumentRangeFormattingClientCapabilities(TypedDict):
    """Client capabilities of a {@link DocumentRangeFormattingRequest}."""

    dynamicRegistration: NotRequired[bool]
    """ Whether range formatting supports dynamic registration. """


class DocumentOnTypeFormattingClientCapabilities(TypedDict):
    """Client capabilities of a {@link DocumentOnTypeFormattingRequest}."""

    dynamicRegistration: NotRequired[bool]
    """ Whether on type formatting supports dynamic registration. """


class RenameClientCapabilities(TypedDict):
    dynamicRegistration: NotRequired[bool]
    """ Whether rename supports dynamic registration. """
    prepareSupport: NotRequired[bool]
    """ Client supports testing for validity of rename operations
    before execution.

    @since 3.12.0 """
    prepareSupportDefaultBehavior: NotRequired["PrepareSupportDefaultBehavior"]
    """ Client supports the default behavior result.

    The value indicates the default behavior used by the
    client.

    @since 3.16.0 """
    honorsChangeAnnotations: NotRequired[bool]
    """ Whether the client honors the change annotations in
    text edits and resource operations returned via the
    rename request's workspace edit by for example presenting
    the workspace edit in the user interface and asking
    for confirmation.

    @since 3.16.0 """


class FoldingRangeClientCapabilities(TypedDict):
    dynamicRegistration: NotRequired[bool]
    """ Whether implementation supports dynamic registration for folding range
    providers. If this is set to `true` the client supports the new
    `FoldingRangeRegistrationOptions` return value for the corresponding
    server capability as well. """
    rangeLimit: NotRequired[Uint]
    """ The maximum number of folding ranges that the client prefers to receive
    per document. The value serves as a hint, servers are free to follow the
    limit. """
    lineFoldingOnly: NotRequired[bool]
    """ If set, the client signals that it only supports folding complete lines.
    If set, client will ignore specified `startCharacter` and `endCharacter`
    properties in a FoldingRange. """
    foldingRangeKind: NotRequired["__FoldingRangeClientCapabilities_foldingRangeKind_Type_1"]
    """ Specific options for the folding range kind.

    @since 3.17.0 """
    foldingRange: NotRequired["__FoldingRangeClientCapabilities_foldingRange_Type_1"]
    """ Specific options for the folding range.

    @since 3.17.0 """


class SelectionRangeClientCapabilities(TypedDict):
    dynamicRegistration: NotRequired[bool]
    """ Whether implementation supports dynamic registration for selection range providers. If this is set to `true`
    the client supports the new `SelectionRangeRegistrationOptions` return value for the corresponding server
    capability as well. """


class PublishDiagnosticsClientCapabilities(TypedDict):
    """The publish diagnostic client capabilities."""

    relatedInformation: NotRequired[bool]
    """ Whether the clients accepts diagnostics with related information. """
    tagSupport: NotRequired["__PublishDiagnosticsClientCapabilities_tagSupport_Type_1"]
    """ Client supports the tag property to provide meta data about a diagnostic.
    Clients supporting tags have to handle unknown tags gracefully.

    @since 3.15.0 """
    versionSupport: NotRequired[bool]
    """ Whether the client interprets the version property of the
    `textDocument/publishDiagnostics` notification's parameter.

    @since 3.15.0 """
    codeDescriptionSupport: NotRequired[bool]
    """ Client supports a codeDescription property

    @since 3.16.0 """
    dataSupport: NotRequired[bool]
    """ Whether code action supports the `data` property which is
    preserved between a `textDocument/publishDiagnostics` and
    `textDocument/codeAction` request.

    @since 3.16.0 """


class CallHierarchyClientCapabilities(TypedDict):
    """@since 3.16.0"""

    dynamicRegistration: NotRequired[bool]
    """ Whether implementation supports dynamic registration. If this is set to `true`
    the client supports the new `(TextDocumentRegistrationOptions & StaticRegistrationOptions)`
    return value for the corresponding server capability as well. """


class SemanticTokensClientCapabilities(TypedDict):
    """@since 3.16.0"""

    dynamicRegistration: NotRequired[bool]
    """ Whether implementation supports dynamic registration. If this is set to `true`
    the client supports the new `(TextDocumentRegistrationOptions & StaticRegistrationOptions)`
    return value for the corresponding server capability as well. """
    requests: "__SemanticTokensClientCapabilities_requests_Type_1"
    """ Which requests the client supports and might send to the server
    depending on the server's capability. Please note that clients might not
    show semantic tokens or degrade some of the user experience if a range
    or full request is advertised by the client but not provided by the
    server. If for example the client capability `requests.full` and
    `request.range` are both set to true but the server only provides a
    range provider the client might not render a minimap correctly or might
    even decide to not show any semantic tokens at all. """
    tokenTypes: List[str]
    """ The token types that the client supports. """
    tokenModifiers: List[str]
    """ The token modifiers that the client supports. """
    formats: List["TokenFormat"]
    """ The token formats the clients supports. """
    overlappingTokenSupport: NotRequired[bool]
    """ Whether the client supports tokens that can overlap each other. """
    multilineTokenSupport: NotRequired[bool]
    """ Whether the client supports tokens that can span multiple lines. """
    serverCancelSupport: NotRequired[bool]
    """ Whether the client allows the server to actively cancel a
    semantic token request, e.g. supports returning
    LSPErrorCodes.ServerCancelled. If a server does the client
    needs to retrigger the request.

    @since 3.17.0 """
    augmentsSyntaxTokens: NotRequired[bool]
    """ Whether the client uses semantic tokens to augment existing
    syntax tokens. If set to `true` client side created syntax
    tokens and semantic tokens are both used for colorization. If
    set to `false` the client only uses the returned semantic tokens
    for colorization.

    If the value is `undefined` then the client behavior is not
    specified.

    @since 3.17.0 """


class LinkedEditingRangeClientCapabilities(TypedDict):
    """Client capabilities for the linked editing range request.

    @since 3.16.0"""

    dynamicRegistration: NotRequired[bool]
    """ Whether implementation supports dynamic registration. If this is set to `true`
    the client supports the new `(TextDocumentRegistrationOptions & StaticRegistrationOptions)`
    return value for the corresponding server capability as well. """


class MonikerClientCapabilities(TypedDict):
    """Client capabilities specific to the moniker request.

    @since 3.16.0"""

    dynamicRegistration: NotRequired[bool]
    """ Whether moniker supports dynamic registration. If this is set to `true`
    the client supports the new `MonikerRegistrationOptions` return value
    for the corresponding server capability as well. """


class TypeHierarchyClientCapabilities(TypedDict):
    """@since 3.17.0"""

    dynamicRegistration: NotRequired[bool]
    """ Whether implementation supports dynamic registration. If this is set to `true`
    the client supports the new `(TextDocumentRegistrationOptions & StaticRegistrationOptions)`
    return value for the corresponding server capability as well. """


class InlineValueClientCapabilities(TypedDict):
    """Client capabilities specific to inline values.

    @since 3.17.0"""

    dynamicRegistration: NotRequired[bool]
    """ Whether implementation supports dynamic registration for inline value providers. """


class InlayHintClientCapabilities(TypedDict):
    """Inlay hint client capabilities.

    @since 3.17.0"""

    dynamicRegistration: NotRequired[bool]
    """ Whether inlay hints support dynamic registration. """
    resolveSupport: NotRequired["__InlayHintClientCapabilities_resolveSupport_Type_1"]
    """ Indicates which properties a client can resolve lazily on an inlay
    hint. """


class DiagnosticClientCapabilities(TypedDict):
    """Client capabilities specific to diagnostic pull requests.

    @since 3.17.0"""

    dynamicRegistration: NotRequired[bool]
    """ Whether implementation supports dynamic registration. If this is set to `true`
    the client supports the new `(TextDocumentRegistrationOptions & StaticRegistrationOptions)`
    return value for the corresponding server capability as well. """
    relatedDocumentSupport: NotRequired[bool]
    """ Whether the clients supports related documents for document diagnostic pulls. """


class NotebookDocumentSyncClientCapabilities(TypedDict):
    """Notebook specific client capabilities.

    @since 3.17.0"""

    dynamicRegistration: NotRequired[bool]
    """ Whether implementation supports dynamic registration. If this is
    set to `true` the client supports the new
    `(TextDocumentRegistrationOptions & StaticRegistrationOptions)`
    return value for the corresponding server capability as well. """
    executionSummarySupport: NotRequired[bool]
    """ The client supports sending execution summary data per cell. """


class ShowMessageRequestClientCapabilities(TypedDict):
    """Show message request client capabilities"""

    messageActionItem: NotRequired["__ShowMessageRequestClientCapabilities_messageActionItem_Type_1"]
    """ Capabilities specific to the `MessageActionItem` type. """


class ShowDocumentClientCapabilities(TypedDict):
    """Client capabilities for the showDocument request.

    @since 3.16.0"""

    support: bool
    """ The client has support for the showDocument
    request. """


class RegularExpressionsClientCapabilities(TypedDict):
    """Client capabilities specific to regular expressions.

    @since 3.16.0"""

    engine: str
    """ The engine's name. """
    version: NotRequired[str]
    """ The engine's version. """


class MarkdownClientCapabilities(TypedDict):
    """Client capabilities specific to the used markdown parser.

    @since 3.16.0"""

    parser: str
    """ The name of the parser. """
    version: NotRequired[str]
    """ The version of the parser. """
    allowedTags: NotRequired[List[str]]
    """ A list of HTML tags that the client allows / supports in
    Markdown.

    @since 3.17.0 """


class __CodeActionClientCapabilities_codeActionLiteralSupport_Type_1(TypedDict):
    codeActionKind: "__CodeActionClientCapabilities_codeActionLiteralSupport_codeActionKind_Type_1"
    """ The code action kind is support with the following value
    set. """


class __CodeActionClientCapabilities_codeActionLiteralSupport_codeActionKind_Type_1(TypedDict):
    valueSet: List["CodeActionKind"]
    """ The code action kind values the client supports. When this
    property exists the client also guarantees that it will
    handle values outside its set gracefully and falls back
    to a default value when unknown. """


class __CodeActionClientCapabilities_resolveSupport_Type_1(TypedDict):
    properties: List[str]
    """ The properties that a client can resolve lazily. """


class __CodeAction_disabled_Type_1(TypedDict):
    reason: str
    """ Human readable description of why the code action is currently disabled.

    This is displayed in the code actions UI. """


class __CompletionClientCapabilities_completionItemKind_Type_1(TypedDict):
    valueSet: NotRequired[List["CompletionItemKind"]]
    """ The completion item kind values the client supports. When this
    property exists the client also guarantees that it will
    handle values outside its set gracefully and falls back
    to a default value when unknown.

    If this property is not present the client only supports
    the completion items kinds from `Text` to `Reference` as defined in
    the initial version of the protocol. """


class __CompletionClientCapabilities_completionItem_Type_1(TypedDict):
    snippetSupport: NotRequired[bool]
    """ Client supports snippets as insert text.

    A snippet can define tab stops and placeholders with `$1`, `$2`
    and `${3:foo}`. `$0` defines the final tab stop, it defaults to
    the end of the snippet. Placeholders with equal identifiers are linked,
    that is typing in one will update others too. """
    commitCharactersSupport: NotRequired[bool]
    """ Client supports commit characters on a completion item. """
    documentationFormat: NotRequired[List["MarkupKind"]]
    """ Client supports the following content formats for the documentation
    property. The order describes the preferred format of the client. """
    deprecatedSupport: NotRequired[bool]
    """ Client supports the deprecated property on a completion item. """
    preselectSupport: NotRequired[bool]
    """ Client supports the preselect property on a completion item. """
    tagSupport: NotRequired["__CompletionClientCapabilities_completionItem_tagSupport_Type_1"]
    """ Client supports the tag property on a completion item. Clients supporting
    tags have to handle unknown tags gracefully. Clients especially need to
    preserve unknown tags when sending a completion item back to the server in
    a resolve call.

    @since 3.15.0 """
    insertReplaceSupport: NotRequired[bool]
    """ Client support insert replace edit to control different behavior if a
    completion item is inserted in the text or should replace text.

    @since 3.16.0 """
    resolveSupport: NotRequired["__CompletionClientCapabilities_completionItem_resolveSupport_Type_1"]
    """ Indicates which properties a client can resolve lazily on a completion
    item. Before version 3.16.0 only the predefined properties `documentation`
    and `details` could be resolved lazily.

    @since 3.16.0 """
    insertTextModeSupport: NotRequired["__CompletionClientCapabilities_completionItem_insertTextModeSupport_Type_1"]
    """ The client supports the `insertTextMode` property on
    a completion item to override the whitespace handling mode
    as defined by the client (see `insertTextMode`).

    @since 3.16.0 """
    labelDetailsSupport: NotRequired[bool]
    """ The client has support for completion item label
    details (see also `CompletionItemLabelDetails`).

    @since 3.17.0 """


class __CompletionClientCapabilities_completionItem_insertTextModeSupport_Type_1(TypedDict):
    valueSet: List["InsertTextMode"]


class __CompletionClientCapabilities_completionItem_resolveSupport_Type_1(TypedDict):
    properties: List[str]
    """ The properties that a client can resolve lazily. """


class __CompletionClientCapabilities_completionItem_tagSupport_Type_1(TypedDict):
    valueSet: List["CompletionItemTag"]
    """ The tags supported by the client. """


class __CompletionClientCapabilities_completionList_Type_1(TypedDict):
    itemDefaults: NotRequired[List[str]]
    """ The client supports the following itemDefaults on
    a completion list.

    The value lists the supported property names of the
    `CompletionList.itemDefaults` object. If omitted
    no properties are supported.

    @since 3.17.0 """


class __CompletionList_itemDefaults_Type_1(TypedDict):
    commitCharacters: NotRequired[List[str]]
    """ A default commit character set.

    @since 3.17.0 """
    editRange: NotRequired[Union["Range", "__CompletionList_itemDefaults_editRange_Type_1"]]
    """ A default edit range.

    @since 3.17.0 """
    insertTextFormat: NotRequired["InsertTextFormat"]
    """ A default insert text format.

    @since 3.17.0 """
    insertTextMode: NotRequired["InsertTextMode"]
    """ A default insert text mode.

    @since 3.17.0 """
    data: NotRequired["LSPAny"]
    """ A default data value.

    @since 3.17.0 """


class __CompletionList_itemDefaults_editRange_Type_1(TypedDict):
    insert: "Range"
    replace: "Range"


class __CompletionOptions_completionItem_Type_1(TypedDict):
    labelDetailsSupport: NotRequired[bool]
    """ The server has support for completion item label
    details (see also `CompletionItemLabelDetails`) when
    receiving a completion item in a resolve call.

    @since 3.17.0 """


class __CompletionOptions_completionItem_Type_2(TypedDict):
    labelDetailsSupport: NotRequired[bool]
    """ The server has support for completion item label
    details (see also `CompletionItemLabelDetails`) when
    receiving a completion item in a resolve call.

    @since 3.17.0 """


class __DocumentSymbolClientCapabilities_symbolKind_Type_1(TypedDict):
    valueSet: NotRequired[List["SymbolKind"]]
    """ The symbol kind values the client supports. When this
    property exists the client also guarantees that it will
    handle values outside its set gracefully and falls back
    to a default value when unknown.

    If this property is not present the client only supports
    the symbol kinds from `File` to `Array` as defined in
    the initial version of the protocol. """


class __DocumentSymbolClientCapabilities_tagSupport_Type_1(TypedDict):
    valueSet: List["SymbolTag"]
    """ The tags supported by the client. """


class __FoldingRangeClientCapabilities_foldingRangeKind_Type_1(TypedDict):
    valueSet: NotRequired[List["FoldingRangeKind"]]
    """ The folding range kind values the client supports. When this
    property exists the client also guarantees that it will
    handle values outside its set gracefully and falls back
    to a default value when unknown. """


class __FoldingRangeClientCapabilities_foldingRange_Type_1(TypedDict):
    collapsedText: NotRequired[bool]
    """ If set, the client signals that it supports setting collapsedText on
    folding ranges to display custom labels instead of the default text.

    @since 3.17.0 """


class __GeneralClientCapabilities_staleRequestSupport_Type_1(TypedDict):
    cancel: bool
    """ The client will actively cancel the request. """
    retryOnContentModified: List[str]
    """ The list of requests for which the client
    will retry the request if it receives a
    response with error code `ContentModified` """


class __InitializeResult_serverInfo_Type_1(TypedDict):
    name: str
    """ The name of the server as defined by the server. """
    version: NotRequired[str]
    """ The server's version as defined by the server. """


class __InlayHintClientCapabilities_resolveSupport_Type_1(TypedDict):
    properties: List[str]
    """ The properties that a client can resolve lazily. """


class __MarkedString_Type_1(TypedDict):
    language: str
    value: str


class __NotebookDocumentChangeEvent_cells_Type_1(TypedDict):
    structure: NotRequired["__NotebookDocumentChangeEvent_cells_structure_Type_1"]
    """ Changes to the cell structure to add or
    remove cells. """
    data: NotRequired[List["NotebookCell"]]
    """ Changes to notebook cells properties like its
    kind, execution summary or metadata. """
    textContent: NotRequired[List["__NotebookDocumentChangeEvent_cells_textContent_Type_1"]]
    """ Changes to the text content of notebook cells. """


class __NotebookDocumentChangeEvent_cells_structure_Type_1(TypedDict):
    array: "NotebookCellArrayChange"
    """ The change to the cell array. """
    didOpen: NotRequired[List["TextDocumentItem"]]
    """ Additional opened cell text documents. """
    didClose: NotRequired[List["TextDocumentIdentifier"]]
    """ Additional closed cell text documents. """


class __NotebookDocumentChangeEvent_cells_textContent_Type_1(TypedDict):
    document: "VersionedTextDocumentIdentifier"
    changes: List["TextDocumentContentChangeEvent"]


class __NotebookDocumentFilter_Type_1(TypedDict):
    notebookType: str
    """ The type of the enclosing notebook. """
    scheme: NotRequired[str]
    """ A Uri {@link Uri.scheme scheme}, like `file` or `untitled`. """
    pattern: NotRequired[str]
    """ A glob pattern. """


class __NotebookDocumentFilter_Type_2(TypedDict):
    notebookType: NotRequired[str]
    """ The type of the enclosing notebook. """
    scheme: str
    """ A Uri {@link Uri.scheme scheme}, like `file` or `untitled`. """
    pattern: NotRequired[str]
    """ A glob pattern. """


class __NotebookDocumentFilter_Type_3(TypedDict):
    notebookType: NotRequired[str]
    """ The type of the enclosing notebook. """
    scheme: NotRequired[str]
    """ A Uri {@link Uri.scheme scheme}, like `file` or `untitled`. """
    pattern: str
    """ A glob pattern. """


class __NotebookDocumentSyncOptions_notebookSelector_Type_1(TypedDict):
    notebook: Union[str, "NotebookDocumentFilter"]
    """ The notebook to be synced If a string
    value is provided it matches against the
    notebook type. '*' matches every notebook. """
    cells: NotRequired[List["__NotebookDocumentSyncOptions_notebookSelector_cells_Type_1"]]
    """ The cells of the matching notebook to be synced. """


class __NotebookDocumentSyncOptions_notebookSelector_Type_2(TypedDict):
    notebook: NotRequired[Union[str, "NotebookDocumentFilter"]]
    """ The notebook to be synced If a string
    value is provided it matches against the
    notebook type. '*' matches every notebook. """
    cells: List["__NotebookDocumentSyncOptions_notebookSelector_cells_Type_2"]
    """ The cells of the matching notebook to be synced. """


class __NotebookDocumentSyncOptions_notebookSelector_Type_3(TypedDict):
    notebook: Union[str, "NotebookDocumentFilter"]
    """ The notebook to be synced If a string
    value is provided it matches against the
    notebook type. '*' matches every notebook. """
    cells: NotRequired[List["__NotebookDocumentSyncOptions_notebookSelector_cells_Type_3"]]
    """ The cells of the matching notebook to be synced. """


class __NotebookDocumentSyncOptions_notebookSelector_Type_4(TypedDict):
    notebook: NotRequired[Union[str, "NotebookDocumentFilter"]]
    """ The notebook to be synced If a string
    value is provided it matches against the
    notebook type. '*' matches every notebook. """
    cells: List["__NotebookDocumentSyncOptions_notebookSelector_cells_Type_4"]
    """ The cells of the matching notebook to be synced. """


class __NotebookDocumentSyncOptions_notebookSelector_cells_Type_1(TypedDict):
    language: str


class __NotebookDocumentSyncOptions_notebookSelector_cells_Type_2(TypedDict):
    language: str


class __NotebookDocumentSyncOptions_notebookSelector_cells_Type_3(TypedDict):
    language: str


class __NotebookDocumentSyncOptions_notebookSelector_cells_Type_4(TypedDict):
    language: str


class __PrepareRenameResult_Type_1(TypedDict):
    range: "Range"
    placeholder: str


class __PrepareRenameResult_Type_2(TypedDict):
    defaultBehavior: bool


class __PublishDiagnosticsClientCapabilities_tagSupport_Type_1(TypedDict):
    valueSet: List["DiagnosticTag"]
    """ The tags supported by the client. """


class __SemanticTokensClientCapabilities_requests_Type_1(TypedDict):
    range: NotRequired[Union[bool, dict]]
    """ The client will send the `textDocument/semanticTokens/range` request if
    the server provides a corresponding handler. """
    full: NotRequired[Union[bool, "__SemanticTokensClientCapabilities_requests_full_Type_1"]]
    """ The client will send the `textDocument/semanticTokens/full` request if
    the server provides a corresponding handler. """


class __SemanticTokensClientCapabilities_requests_full_Type_1(TypedDict):
    delta: NotRequired[bool]
    """ The client will send the `textDocument/semanticTokens/full/delta` request if
    the server provides a corresponding handler. """


class __SemanticTokensOptions_full_Type_1(TypedDict):
    delta: NotRequired[bool]
    """ The server supports deltas for full documents. """


class __SemanticTokensOptions_full_Type_2(TypedDict):
    delta: NotRequired[bool]
    """ The server supports deltas for full documents. """


class __ServerCapabilities_workspace_Type_1(TypedDict):
    workspaceFolders: NotRequired["WorkspaceFoldersServerCapabilities"]
    """ The server supports workspace folder.

    @since 3.6.0 """
    fileOperations: NotRequired["FileOperationOptions"]
    """ The server is interested in notifications/requests for operations on files.

    @since 3.16.0 """


class __ShowMessageRequestClientCapabilities_messageActionItem_Type_1(TypedDict):
    additionalPropertiesSupport: NotRequired[bool]
    """ Whether the client supports additional attributes which
    are preserved and send back to the server in the
    request's response. """


class __SignatureHelpClientCapabilities_signatureInformation_Type_1(TypedDict):
    documentationFormat: NotRequired[List["MarkupKind"]]
    """ Client supports the following content formats for the documentation
    property. The order describes the preferred format of the client. """
    parameterInformation: NotRequired[
        "__SignatureHelpClientCapabilities_signatureInformation_parameterInformation_Type_1"
    ]
    """ Client capabilities specific to parameter information. """
    activeParameterSupport: NotRequired[bool]
    """ The client supports the `activeParameter` property on `SignatureInformation`
    literal.

    @since 3.16.0 """


class __SignatureHelpClientCapabilities_signatureInformation_parameterInformation_Type_1(TypedDict):
    labelOffsetSupport: NotRequired[bool]
    """ The client supports processing label offsets instead of a
    simple label string.

    @since 3.14.0 """


class __TextDocumentContentChangeEvent_Type_1(TypedDict):
    range: "Range"
    """ The range of the document that changed. """
    rangeLength: NotRequired[Uint]
    """ The optional length of the range that got replaced.

    @deprecated use range instead. """
    text: str
    """ The new text for the provided range. """


class __TextDocumentContentChangeEvent_Type_2(TypedDict):
    text: str
    """ The new text of the whole document. """


class __TextDocumentFilter_Type_1(TypedDict):
    language: str
    """ A language id, like `typescript`. """
    scheme: NotRequired[str]
    """ A Uri {@link Uri.scheme scheme}, like `file` or `untitled`. """
    pattern: NotRequired[str]
    """ A glob pattern, like `*.{ts,js}`. """


class __TextDocumentFilter_Type_2(TypedDict):
    language: NotRequired[str]
    """ A language id, like `typescript`. """
    scheme: str
    """ A Uri {@link Uri.scheme scheme}, like `file` or `untitled`. """
    pattern: NotRequired[str]
    """ A glob pattern, like `*.{ts,js}`. """


class __TextDocumentFilter_Type_3(TypedDict):
    language: NotRequired[str]
    """ A language id, like `typescript`. """
    scheme: NotRequired[str]
    """ A Uri {@link Uri.scheme scheme}, like `file` or `untitled`. """
    pattern: str
    """ A glob pattern, like `*.{ts,js}`. """


class __WorkspaceEditClientCapabilities_changeAnnotationSupport_Type_1(TypedDict):
    groupsOnLabel: NotRequired[bool]
    """ Whether the client groups edits with equal labels into tree nodes,
    for instance all edits labelled with "Changes in Strings" would
    be a tree node. """


class __WorkspaceSymbolClientCapabilities_resolveSupport_Type_1(TypedDict):
    properties: List[str]
    """ The properties that a client can resolve lazily. Usually
    `location.range` """


class __WorkspaceSymbolClientCapabilities_symbolKind_Type_1(TypedDict):
    valueSet: NotRequired[List["SymbolKind"]]
    """ The symbol kind values the client supports. When this
    property exists the client also guarantees that it will
    handle values outside its set gracefully and falls back
    to a default value when unknown.

    If this property is not present the client only supports
    the symbol kinds from `File` to `Array` as defined in
    the initial version of the protocol. """


class __WorkspaceSymbolClientCapabilities_tagSupport_Type_1(TypedDict):
    valueSet: List["SymbolTag"]
    """ The tags supported by the client. """


class __WorkspaceSymbol_location_Type_1(TypedDict):
    uri: "DocumentUri"


class ___InitializeParams_clientInfo_Type_1(TypedDict):
    name: str
    """ The name of the client as defined by the client. """
    version: NotRequired[str]
    """ The client's version as defined by the client. """
