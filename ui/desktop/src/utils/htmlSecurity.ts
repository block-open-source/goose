/**
 * HTML Security Detection Utilities
 *
 * These functions detect potentially dangerous HTML content in markdown
 * and wrap it safely in code blocks to prevent execution.
 */

/**
 * Detects if content contains potentially dangerous HTML
 * @param str - The content to check
 * @returns true if dangerous HTML is detected
 */
export function containsHTML(str: string): boolean {
  // Remove fenced code blocks and inline code first
  const withoutCodeBlocks = str.replace(/```[\s\S]*?```/g, '').replace(/`[^`]*`/g, '');

  // Check for HTML comments first
  const commentStart = withoutCodeBlocks.indexOf('<!--');
  const hasComments =
    commentStart !== -1 && withoutCodeBlocks.indexOf('-->', commentStart + 4) !== -1;

  // Only detect potentially dangerous HTML tags that could execute or affect layout
  const dangerousHTMLRegex =
    /<(script|style|iframe|object|embed|form|input|button|link|meta|base|br|hr|img|div|span|p|h[1-6]|a|strong|em|b|i|u|s|pre|code|blockquote|section|article|header|footer|nav|aside|main|table|tr|td|th|ul|ol|li)(?:\s[^>]*)?(?:\s*\/?>|>[^<]*<\/\1>)/i;
  const hasDangerousHTML = dangerousHTMLRegex.test(withoutCodeBlocks);

  return hasComments || hasDangerousHTML;
}

/**
 * Wraps HTML content in code blocks for safe display
 * @param content - The markdown content to process
 * @returns Processed content with HTML wrapped in code blocks
 */
export function wrapHTMLInCodeBlock(content: string): string {
  const lines = content.split('\n');
  let insideCodeBlock = false;

  const processedLines = lines.map((line) => {
    // Track if we're inside a code block
    if (line.trim().startsWith('```')) {
      insideCodeBlock = !insideCodeBlock;
      return line;
    }

    // If we're inside a code block, don't process the content - just leave it as-is
    if (insideCodeBlock) {
      return line;
    }

    // Only check for HTML in lines that are NOT inside code blocks
    if (containsHTML(line)) {
      return `\`\`\`html\n${line}\n\`\`\``;
    }

    return line;
  });

  return processedLines.join('\n');
}
