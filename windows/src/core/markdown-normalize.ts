const codeRegion = /(`{3,}|~{3,})[\s\S]*?(?:\1|$)|(`+)[^\n]*?(?:\2|$)/g;

function transformOutsideCode(markdownText: string, transform: (text: string) => string): string {
  const codeSegments: string[] = [];
  const protectedText = markdownText.replace(codeRegion, (code) => {
    const index = codeSegments.push(code) - 1;
    return `\u0000${index}\u0000`;
  });
  return transform(protectedText).replace(/\u0000(\d+)\u0000/g, (_match, index: string) => {
    return codeSegments[Number(index)] ?? "";
  });
}

function transformOutsideMath(markdownText: string, transform: (text: string) => string): string {
  const mathSegments: string[] = [];
  const protectedText = markdownText.replace(/\$\$[\s\S]+?\$\$|\$(?:\\.|[^$\n])+?\$/g, (math) => {
    const index = mathSegments.push(math) - 1;
    return `\u0001${index}\u0001`;
  });
  return transform(protectedText).replace(/\u0001(\d+)\u0001/g, (_match, index: string) => {
    return mathSegments[Number(index)] ?? "";
  });
}

function separateInlineBlocks(markdownText: string): string {
  return markdownText
    .replace(/([^\n])([ \t]+)(?=(?:\d{1,2}\.|[-*+])\s+(?:\S))/g, "$1\n\n")
    .replace(/([^\n])([ \t]+)(?=#{1,6}\s)/g, "$1\n\n");
}

export function normalizeAssistantMarkdown(markdownText: string): string {
  const normalized = markdownText.replace(/\r\n?/g, "\n");
  return transformOutsideCode(normalized, (text) => transformOutsideMath(
    text.replace(
      /\\\[([\s\S]+?)\\\]|\\\(([\s\S]+?)\\\)/g,
      (match, display: string | undefined, inline: string | undefined) =>
        display !== undefined ? `$$${display}$$` : `$${inline ?? match}$`,
    ),
    separateInlineBlocks,
  ));
}
