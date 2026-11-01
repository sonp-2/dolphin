// 実行はここ（Web Worker）で行う。無限ループでもページは固まらず、親が terminate で止められる
import init, { run } from './wasm/dolphin_web.js'

const ready = init()

self.onmessage = async ({ data }) => {
  await ready
  const r = run(data)
  self.postMessage({ output: r.output, error: r.error, line: r.line })
}
