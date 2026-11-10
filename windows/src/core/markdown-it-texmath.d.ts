declare module "markdown-it-texmath" {
  import type MarkdownIt from "markdown-it";

  interface TexmathOptions {
    engine: {
      renderToString: (
        tex: string,
        options?: import("katex").KatexOptions,
      ) => string;
    };
    delimiters: "dollars" | "brackets" | "doxygen" | "gitlab" | "julia" | "kramdown" | "beg_end";
    katexOptions?: Record<string, unknown>;
  }

  const texmath: (markdown: MarkdownIt, options: TexmathOptions) => void;
  export default texmath;
}
