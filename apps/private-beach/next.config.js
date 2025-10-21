/** @type {import('next').NextConfig} */
/** @type {import('next').NextConfig} */
const nextConfig = {
  reactStrictMode: true,
  swcMinify: true,
  experimental: {
    externalDir: true,
  },
  webpack: (config, { isServer }) => {
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
    const noiseWasmPath = require.resolve('noise-c.wasm/src/noise-c.wasm');
    config.resolve.alias = {
      ...(config.resolve.alias || {}),
      'noise-c.wasm/src/noise-c.wasm': noiseWasmPath,
      react: path.resolve(__dirname, 'node_modules/react'),
      'react-dom': path.resolve(__dirname, 'node_modules/react-dom'),
    };

    return config;
  },
};

module.exports = nextConfig;
