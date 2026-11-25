/// <reference types="vite/client" />

/** 仅含版本类型、应用版本号和提交短哈希，由 Vite 在构建时写入。 */
declare const __ASSEMBLE_BUILD__: import("./lib/build-info").BuildInfo;
