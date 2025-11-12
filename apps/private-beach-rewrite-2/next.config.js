const describeEnv = () => {
  const summary = {
    NEXT_PUBLIC_PRIVATE_BEACH_MANAGER_URL:
      process.env.NEXT_PUBLIC_PRIVATE_BEACH_MANAGER_URL ??
      process.env.NEXT_PUBLIC_MANAGER_URL ??
      '(unset)',
    NEXT_PUBLIC_PRIVATE_BEACH_REWRITE_ENABLED:
      process.env.NEXT_PUBLIC_PRIVATE_BEACH_REWRITE_ENABLED ?? '(unset)',
    NEXT_PUBLIC_CLERK_MANAGER_TOKEN_TEMPLATE: Boolean(
      process.env.NEXT_PUBLIC_CLERK_MANAGER_TOKEN_TEMPLATE && process.env.NEXT_PUBLIC_CLERK_MANAGER_TOKEN_TEMPLATE.trim().length > 0,
    ),
    NEXT_PUBLIC_PRIVATE_BEACH_TERMINAL_TRACE:
      process.env.NEXT_PUBLIC_PRIVATE_BEACH_TERMINAL_TRACE ?? '0',
  };
  // eslint-disable-next-line no-console
  console.info('[private-beach-rewrite-config]', summary);
};

describeEnv();

/** @type {import('next').NextConfig} */
const nextConfig = {
  reactStrictMode: true,
  experimental: {
    externalDir: true,
  },
  webpack: (config) => {
    config.experiments = {
      ...(config.experiments || {}),
      asyncWebAssembly: true,
    };

    config.resolve = config.resolve || {};
    config.resolve.fallback = {
      ...(config.resolve.fallback || {}),
      fs: false,
      path: false,
      url: false,
    };

    config.module.rules = config.module.rules || [];
    config.module.rules.push(
      {
        test: /\.wasm$/i,
        resourceQuery: /url/,
        type: 'asset/resource',
      },
      {
        test: /\.wasm$/i,
        type: 'webassembly/async',
      },
    );

    const path = require('path');
    let noiseWasmPath;
    try {
      noiseWasmPath = require.resolve('noise-c.wasm/src/noise-c.wasm');
    } catch {
      noiseWasmPath = path.resolve(__dirname, '../private-beach/node_modules/noise-c.wasm/src/noise-c.wasm');
    }
    config.resolve.alias = {
      ...(config.resolve.alias || {}),
      'noise-c.wasm/src/noise-c.wasm': noiseWasmPath,
    };

    return config;
  },
};

module.exports = nextConfig;
