#!/usr/bin/env node

/**
 * Test script for Chrome DevTools MCP
 * Run with: node test-chrome-mcp.js
 */

const { spawn } = require('child_process');

console.log('Testing Chrome DevTools MCP installation...\n');

// Test that npx can run the chrome-devtools-mcp
const chromeDevTools = spawn('npx', ['-y', 'chrome-devtools-mcp@latest', '--help'], {
  stdio: 'pipe'
});

let output = '';

chromeDevTools.stdout.on('data', (data) => {
  output += data.toString();
});

chromeDevTools.stderr.on('data', (data) => {
  console.error(`Error: ${data}`);
});

chromeDevTools.on('close', (code) => {
  if (code === 0 || output.includes('chrome-devtools-mcp')) {
    console.log('✅ Chrome DevTools MCP is available and can be executed');
    console.log('\n📋 Configuration has been added to:');
    console.log('   - Claude Code: ~/Library/Application Support/Claude/claude_desktop_config.json');
    console.log('   - Generic config: ./chrome-devtools-mcp-config.json');
    console.log('\n🔄 Next steps:');
    console.log('   1. Restart Claude Code to load the new MCP server');
    console.log('   2. For Codex/VS Code, add the config from chrome-devtools-mcp-config.json to your IDE\'s MCP settings');
    console.log('\n📚 Available Chrome DevTools MCP features:');
    console.log('   - Browser automation and control');
    console.log('   - Performance profiling');
    console.log('   - Network inspection');
    console.log('   - Console access');
    console.log('   - Element inspection');
  } else {
    console.log('❌ Chrome DevTools MCP test failed');
    console.log('Please ensure Node.js and npm are properly installed');
  }
});