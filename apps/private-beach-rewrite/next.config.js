/** @type {import('next').NextConfig} */
const nextConfig = {
  reactStrictMode: true,
  experimental: {
    externalDir: true,
  },
  webpackDevMiddleware: (config) => {
    config.watchOptions = {
      poll: 1000,
      aggregateTimeout: 300,
      ...(config.watchOptions || {}),
    };
    return config;
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
