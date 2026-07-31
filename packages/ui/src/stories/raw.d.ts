// Vite/Ladle `?raw` imports resolve to the file's text content. Declared here so
// the story files type-check; the actual resolution happens in the Ladle/Vite
// dev server.
declare module '*?raw' {
  const content: string;
  export default content;
}
