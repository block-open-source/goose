class LSPConstants:
    """
    This class contains constants used in the LSP protocol.
    """

    # the key for uri used to represent paths
    URI = "uri"

    # the key for range, which is a from and to position within a text document
    RANGE = "range"

    # A key used in LocationLink type, used as the span of the origin link
    ORIGIN_SELECTION_RANGE = "originSelectionRange"

    # A key used in LocationLink type, used as the target uri of the link
    TARGET_URI = "targetUri"

    # A key used in LocationLink type, used as the target range of the link
    TARGET_RANGE = "targetRange"

    # A key used in LocationLink type, used as the target selection range of the link
    TARGET_SELECTION_RANGE = "targetSelectionRange"

    # key for the textDocument field in the request
    TEXT_DOCUMENT = "textDocument"

    # key used to represent the language a document is in - "java", "csharp", etc.
    LANGUAGE_ID = "languageId"

    # key used to represent the version of a document (a shared value betwen the client and server)
    VERSION = "version"

    # key used to represent the text of a document being sent from the client to the server on open
    TEXT = "text"

    # key used to represent a position (line and colnum) within a text document
    POSITION = "position"

    # key used to represent the line number of a position
    LINE = "line"

    # key used to represent the column number of a position
    CHARACTER = "character"

    # key used to represent the changes made to a document
    CONTENT_CHANGES = "contentChanges"

    # key used to represent name of symbols
    NAME = "name"

    # key used to represent the kind of symbols
    KIND = "kind"

    # key used to represent children in document symbols
    CHILDREN = "children"
