"""
Defines wrapper objects around the types returned by LSP to ensure decoupling
between LSP versions and our implementation.

This file is obtained from https://github.com/microsoft/multilspy under the MIT License with the following terms:

MIT License

Copyright (c) Microsoft Corporation.

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
SOFTWARE
"""

from __future__ import annotations

from enum import IntEnum, Enum
from typing import NotRequired, TypedDict, List, Dict, Union

URI = str
DocumentUri = str
Uint = int
RegExp = str


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

    start: Position
    """ The range's start position. """
    end: Position
    """ The range's end position. """


class Location(TypedDict):
    """Represents a location inside a resource, such as a line
    inside a text file."""

    uri: DocumentUri
    range: Range
    absolutePath: str
    relativePath: str


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


class CompletionItem(TypedDict):
    """A completion item represents a text snippet that is
    proposed to complete text that is being typed."""

    completionText: str
    """ The completionText of this completion item.

    The completionText property is also by default the text that
    is inserted when selecting this completion."""

    kind: CompletionItemKind
    """ The kind of this completion item. Based of the kind
    an icon is chosen by the editor. """

    detail: NotRequired[str]
    """ A human-readable string with additional information
    about this item, like type or symbol information. """


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


class UnifiedSymbolInformation(TypedDict):
    """Represents information about programming constructs like variables, classes,
    interfaces etc."""

    deprecated: NotRequired[bool]
    """ Indicates if this symbol is deprecated.

    @deprecated Use tags instead """
    location: NotRequired[Location]
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
    kind: SymbolKind
    """ The kind of this symbol. """
    tags: NotRequired[List[SymbolTag]]
    """ Tags for this symbol.

    @since 3.16.0 """
    containerName: NotRequired[str]
    """ The name of the symbol containing this symbol. This information is for
    user interface purposes (e.g. to render a qualifier in the user interface
    if necessary). It can't be used to re-infer a hierarchy for the document
    symbols. """

    detail: NotRequired[str]
    """ More detail for this symbol, e.g the signature of a function. """

    range: NotRequired[Range]
    """ The range enclosing this symbol not including leading/trailing whitespace but everything else
    like comments. This information is typically used to determine if the clients cursor is
    inside the symbol to reveal in the symbol in the UI. """
    selectionRange: NotRequired[Range]
    """ The range that should be selected and revealed when this symbol is being picked, e.g the name of a function.
    Must be contained by the `range`. """


TreeRepr = Dict[int, List["TreeRepr"]]


class MarkupKind(Enum):
    """Describes the content type that a client supports in various
    result literals like `Hover`, `ParameterInfo` or `CompletionItem`.

    Please note that `MarkupKinds` must not start with a `$`. This kinds
    are reserved for internal usage."""

    PlainText = "plaintext"
    """ Plain text is supported as a content format """
    Markdown = "markdown"
    """ Markdown is supported as a content format """


class __MarkedString_Type_1(TypedDict):
    language: str
    value: str


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


class Hover(TypedDict):
    """The result of a hover request."""

    contents: Union["MarkupContent", "MarkedString", List["MarkedString"]]
    """ The hover's content """
    range: NotRequired["Range"]
    """ An optional range inside the text document that is used to
    visualize the hover, e.g. by changing the background color. """
