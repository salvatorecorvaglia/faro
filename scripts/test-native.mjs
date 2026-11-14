import { spawn } from 'node:child_process';
import { chmod, copyFile, mkdtemp, readFile, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';

const directory = await mkdtemp(path.join(os.tmpdir(), 'faro-native-'));
if (process.env.GITHUB_OUTPUT) {
  const { appendFile } = await import('node:fs/promises');
  await appendFile(process.env.GITHUB_OUTPUT, `artifacts=${directory}\n`);
}
const frontend = path.join(directory, 'frontend');
const config = {
  // A Windows absolute path can deserialize as FrontendDist::Url, causing
  // Tauri to navigate to file:///... instead of embedding the frontend.
  // The Files variant accepts directories recursively and is unambiguous,
  // including when the temp directory is on a different drive from the repo.
  build: { frontendDist: [frontend] },
  app: {
    windows: [
      {
        label: 'main',
        title: 'Faro native validation',
        width: 1400,
        height: 900,
        incognito: true,
        dataDirectory: path.join(directory, 'webview'),
        // Hosted Windows runners do not reliably keep the test window foregrounded.
        // Keep timers and worker-driven checks running when WebView2 is occluded.
        // These replace Wry's defaults, so preserve its SmartScreen/mini-menu flags.
        ...(process.platform === 'win32'
          ? {
              additionalBrowserArgs:
                '--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --autoplay-policy=no-user-gesture-required --disable-background-timer-throttling --disable-backgrounding-occluded-windows --disable-renderer-backgrounding',
            }
          : {}),
      },
    ],
  },
};
const baseEnv = {
  ...process.env,
  CARGO_BUILD_JOBS: process.env.CARGO_BUILD_JOBS ?? '2',
  TAURI_CONFIG: JSON.stringify(config),
};
async function run(command, args, env, timeoutMs = 180_000) {
  const child = spawn(command, args, { env, stdio: ['ignore', 'pipe', 'pipe'] });
  let log = '';
  child.stdout.on('data', (data) => {
    log += data;
  });
  child.stderr.on('data', (data) => {
    log += data;
  });
  let timedOut = false;
  const timeout = setTimeout(() => {
    timedOut = true;
    child.kill('SIGTERM');
  }, timeoutMs);
  const { code, signal } = await new Promise((resolve, reject) => {
    child.on('error', reject);
    child.on('close', (code, signal) => resolve({ code, signal }));
  });
  clearTimeout(timeout);
  return { code, signal, timedOut, log };
}
try {
  const assets = await run(
    process.execPath,
    ['node_modules/vite/bin/vite.js', 'build', '--mode', 'desktop-smoke', '--outDir', frontend],
    process.env,
  );
  if (assets.code !== 0) throw new Error(`Native assets failed:\n${assets.log}`);
  const build = await run(
    'cargo',
    [
      'build',
      '--manifest-path',
      'src-tauri/Cargo.toml',
      '--locked',
      '--offline',
      '--features',
      'desktop-smoke,custom-protocol',
    ],
    baseEnv,
    45 * 60_000,
  );
  if (build.code !== 0) throw new Error(`Native smoke build failed:\n${build.log}`);
  if (process.platform === 'darwin') {
    const keychain = await run(
      'xcrun',
      ['swift', 'tests/native/keychain.swift', directory],
      process.env,
    );
    await writeFile(path.join(directory, 'keychain.log'), keychain.log);
    if (keychain.code !== 0) throw new Error(`Disposable keychain check failed:\n${keychain.log}`);
    console.log(keychain.log.trim().split('\n').at(-1));
  }
  const binaryName = process.platform === 'win32' ? 'faro.exe' : 'faro';
  const binary = path.join(directory, binaryName);
  await copyFile(path.join('src-tauri/target/debug', binaryName), binary);
  await chmod(binary, 0o755);
  for (const action of ['close', 'quit']) {
    const report = path.join(directory, `${action}.json`);
    const native = await run(binary, [], {
      ...baseEnv,
      FARO_SMOKE_REPORT: report,
      FARO_SMOKE_EXIT: action,
    });
    await writeFile(path.join(directory, `${action}.log`), native.log);
    let result;
    try {
      result = JSON.parse(await readFile(report, 'utf8'));
    } catch {
      throw new Error(
        `Native smoke produced no report; code=${native.code}; signal=${native.signal}; timedOut=${native.timedOut}; logs: ${directory}\n${native.log.slice(-6000)}`,
      );
    }
    console.log(JSON.stringify({ directory, action, code: native.code, ...result }, null, 2));
    if (native.code !== 0 || native.timedOut || !result.ok) {
      process.exitCode = 1;
      break;
    }
  }
} finally {
  console.log(`Native validation artifacts: ${directory}`);
}
