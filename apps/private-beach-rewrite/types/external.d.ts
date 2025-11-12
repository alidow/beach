declare const __APP_VERSION__: string;

declare module 'argon2-browser' {
  const argon2: any;
  export default argon2;
}

declare module 'argon2-browser/dist/argon2-bundled.min.js' {
  const argon2: any;
  export default argon2;
}

declare module 'noise-c.wasm' {
  const noise: any;
  export default noise;
}

declare module 'next/dist/compiled/react' {
  export * from 'react';
}
