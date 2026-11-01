// canvas.run の画面。入力を集めて、フレームごとに worker へ送り、返ってきた描画命令を描く

const KEYS = {
  ArrowLeft: 'left', ArrowRight: 'right', ArrowUp: 'up', ArrowDown: 'down',
  ' ': 'space', Enter: 'enter', Escape: 'escape', Tab: 'tab', Backspace: 'backspace',
  Delete: 'delete', Insert: 'insert', Home: 'home', End: 'end',
  PageUp: 'pageup', PageDown: 'pagedown', Shift: 'shift', Control: 'ctrl', Alt: 'alt',
}

function keyName(e) {
  if (KEYS[e.key]) return KEYS[e.key]
  const k = e.key.toLowerCase()
  if (/^[a-z0-9]$/.test(k) || /^f([1-9]|1[0-2])$/.test(k)) return k
  return null
}

const rgba = ([r, g, b, a]) => `rgba(${r},${g},${b},${a / 255})`

export class Screen {
  constructor(canvas) {
    this.canvas = canvas
    this.ctx = canvas.getContext('2d')
    this.running = false
    this.down = new Set()
    this.pressed = new Set()
    this.released = new Set()
    this.mouse = { x: 0, y: 0, inside: false, buttons: 0, pressed: 0, wheel: 0 }

    const stopKeys = (e) => {
      if (this.running && keyName(e)) e.preventDefault()
    }
    canvas.addEventListener('keydown', (e) => {
      const k = keyName(e)
      if (!k) return
      stopKeys(e)
      if (!this.down.has(k)) this.pressed.add(k)
      this.down.add(k)
    })
    canvas.addEventListener('keyup', (e) => {
      const k = keyName(e)
      if (!k) return
      stopKeys(e)
      this.down.delete(k)
      this.released.add(k)
    })
    canvas.addEventListener('blur', () => this.down.clear())
    canvas.addEventListener('pointermove', (e) => this.#point(e))
    canvas.addEventListener('pointerenter', (e) => { this.#point(e); this.mouse.inside = true })
    canvas.addEventListener('pointerleave', () => { this.mouse.inside = false })
    canvas.addEventListener('pointerdown', (e) => {
      canvas.focus()
      this.#point(e)
      const bit = [1, 4, 2][e.button]
      if (bit) { this.mouse.buttons |= bit; this.mouse.pressed |= bit }
    })
    window.addEventListener('pointerup', (e) => {
      const bit = [1, 4, 2][e.button]
      if (bit) this.mouse.buttons &= ~bit
    })
    canvas.addEventListener('wheel', (e) => {
      e.preventDefault()
      this.mouse.wheel += e.deltaY
    }, { passive: false })
  }

  #point(e) {
    const r = this.canvas.getBoundingClientRect()
    this.mouse.x = ((e.clientX - r.left) * this.width) / r.width
    this.mouse.y = ((e.clientY - r.top) * this.height) / r.height
    this.mouse.inside = true
  }

  /** 画面を開く。`send(dt, input)` でフレームを worker に頼む */
  open({ width, height, fps, background }, send) {
    this.width = width
    this.height = height
    this.fps = fps
    this.background = background
    this.send = send
    const dpr = window.devicePixelRatio || 1
    this.canvas.width = Math.round(width * dpr)
    this.canvas.height = Math.round(height * dpr)
    this.canvas.style.setProperty('--ratio', width / height)
    this.scale = (this.canvas.width) / width
    this.running = true
    this.waiting = false
    this.last = performance.now()
    this.canvas.parentElement.hidden = false
    this.paint([], background)
    requestAnimationFrame(this.#tick)
  }

  close() {
    this.running = false
  }

  #tick = (now) => {
    if (!this.running) return
    requestAnimationFrame(this.#tick)
    // 追いつかないときはフレームを溜めずに待つ
    if (this.waiting || now - this.last < 1000 / this.fps - 1) return
    const dt = (now - this.last) / 1000
    this.last = now
    this.waiting = true
    const m = this.mouse
    const input = {
      down: [...this.down].join(' '),
      pressed: [...this.pressed].join(' '),
      released: [...this.released].join(' '),
      mouseX: m.x, mouseY: m.y, inside: m.inside,
      buttons: m.buttons, buttonsPressed: m.pressed, wheel: m.wheel,
    }
    this.pressed.clear()
    this.released.clear()
    m.pressed = 0
    m.wheel = 0
    this.send(dt, input)
  }

  /** worker から届いたフレーム */
  receive(cmdsJson) {
    this.waiting = false
    if (cmdsJson) this.paint(JSON.parse(cmdsJson), this.background)
  }

  paint(cmds, background) {
    const { ctx } = this
    ctx.setTransform(this.scale, 0, 0, this.scale, 0, 0)
    ctx.fillStyle = background
    ctx.fillRect(0, 0, this.width, this.height)
    ctx.textBaseline = 'top'
    for (const c of cmds) {
      switch (c.t) {
        case 'clear':
          ctx.fillStyle = rgba(c.c)
          ctx.fillRect(0, 0, this.width, this.height)
          break
        case 'rect':
          if (c.s == null) { ctx.fillStyle = rgba(c.c); ctx.fillRect(c.x, c.y, c.w, c.h) }
          else { ctx.strokeStyle = rgba(c.c); ctx.lineWidth = c.s; ctx.strokeRect(c.x, c.y, c.w, c.h) }
          break
        case 'circle':
          ctx.beginPath()
          ctx.arc(c.x, c.y, c.r, 0, Math.PI * 2)
          if (c.s == null) { ctx.fillStyle = rgba(c.c); ctx.fill() }
          else { ctx.strokeStyle = rgba(c.c); ctx.lineWidth = c.s; ctx.stroke() }
          break
        case 'line':
          ctx.beginPath()
          ctx.moveTo(c.x1, c.y1)
          ctx.lineTo(c.x2, c.y2)
          ctx.strokeStyle = rgba(c.c)
          ctx.lineWidth = c.w
          ctx.stroke()
          break
        case 'polygon':
          ctx.beginPath()
          c.p.forEach(([x, y], i) => (i ? ctx.lineTo(x, y) : ctx.moveTo(x, y)))
          ctx.closePath()
          if (c.s == null) { ctx.fillStyle = rgba(c.c); ctx.fill() }
          else { ctx.strokeStyle = rgba(c.c); ctx.lineWidth = c.s; ctx.stroke() }
          break
        case 'text':
          ctx.fillStyle = rgba(c.c)
          ctx.font = `${c.z}px system-ui, sans-serif`
          ctx.fillText(c.text, c.x, c.y)
          break
      }
    }
  }
}
