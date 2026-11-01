// 実行はここ（Web Worker）で行う。無限ループでもページは固まらず、親が terminate で止められる
import init, { run, frame, stop } from './wasm/dolphin_web.js'

const ready = init()

self.onmessage = async ({ data }) => {
  await ready
  if (data.type === 'run') {
    const r = run(data.src)
    self.postMessage({
      type: 'result',
      output: r.output, error: r.error, line: r.line,
      canvas: r.canvas, width: r.width, height: r.height, fps: r.fps, background: r.background,
    })
  } else if (data.type === 'frame') {
    const i = data.input
    const f = frame(
      data.dt, i.down, i.pressed, i.released,
      i.mouseX, i.mouseY, i.inside, i.buttons, i.buttonsPressed, i.wheel,
    )
    self.postMessage({ type: 'frame', cmds: f.cmds, output: f.output, error: f.error, quit: f.quit })
  } else if (data.type === 'stop') {
    stop()
  }
}
