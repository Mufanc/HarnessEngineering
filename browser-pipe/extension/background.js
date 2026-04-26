const WS_URL = 'ws://127.0.0.1:10129'
const RECONNECT_BASE_MS = 1000
const RECONNECT_MAX_MS = 30000
const RECONNECT_JITTER_MS = 1000
const FETCH_TIMEOUT_MS = 30000
const KEEPALIVE_ALARM_NAME = 'keepalive'

class BrowserPipe {
    constructor() {
        this.ws = null
        this.reconnectAttempt = 0
        this.reconnectTimer = null
    }

    start() {
        chrome.alarms.create(KEEPALIVE_ALARM_NAME, {periodInMinutes: 0.4})
        chrome.alarms.onAlarm.addListener((alarm) => {
            if (alarm.name !== KEEPALIVE_ALARM_NAME) {
                return
            }

            if (!this.ws || this.ws.readyState !== WebSocket.OPEN) {
                this.connect()
            }
        })

        this.connect()
    }

    connect() {
        if (this.ws && (this.ws.readyState === WebSocket.CONNECTING || this.ws.readyState === WebSocket.OPEN)) {
            return
        }

        try {
            this.ws = new WebSocket(WS_URL)
        } catch (err) {
            console.warn('[pipe] Failed to connect WebSocket:', err)
            this.scheduleReconnect()
            return
        }

        this.ws.onopen = () => {
            console.log('[pipe] Connected to MCP server')
            this.reconnectAttempt = 0
        }

        this.ws.onmessage = (event) => {
            void (this.handleMessage(event.data))
        }

        this.ws.onclose = () => {
            console.log('[pipe] WebSocket closed')
            this.ws = null
            this.scheduleReconnect()
        }

        this.ws.onerror = (err) => {
            console.warn('[pipe] WebSocket error:', err)
        }
    }

    scheduleReconnect() {
        if (this.reconnectTimer) {
            return
        }

        const delay = Math.min(
            RECONNECT_BASE_MS * Math.pow(2, this.reconnectAttempt),
            RECONNECT_MAX_MS
        ) + Math.random() * RECONNECT_JITTER_MS

        this.reconnectAttempt++
        console.log(`[pipe] Reconnecting in ${Math.round(delay)} ms (attempt ${this.reconnectAttempt})`)

        this.reconnectTimer = setTimeout(() => {
            this.reconnectTimer = null
            this.connect()
        }, delay)
    }

    async handleMessage(data) {
        let msg
        try {
            msg = JSON.parse(data)
        } catch (err) {
            console.warn('[pipe] Failed to parse message:', err)
            return
        }

        if (msg.type === 'fetch_request') {
            try {
                const resp = await this.fetch(msg)
                this.send(resp)
            } catch (err) {
                this.send({
                    id: msg.id,
                    type: 'fetch_error',
                    error: err.message || String(err)
                })
            }
        }
    }

    send(msg) {
        if (this.ws && this.ws.readyState === WebSocket.OPEN) {
            this.ws.send(JSON.stringify(msg))
        } else {
            console.warn('[pipe] Cannot send message: WebSocket not open')
        }
    }

    async fetch(req) {
        const headers = new Headers(req.headers || {});

        const controller = new AbortController()
        const timer = setTimeout(() => controller.abort(), FETCH_TIMEOUT_MS)

        try {
            const options = {
                method: req.method || 'GET',
                headers,
                credentials: 'include',
                signal: controller.signal,
                redirect: req.redirect || 'follow'
            }

            if (req.body && !['GET', 'HEAD'].includes(options.method.toUpperCase())) {
                options.body = req.body
            }

            const resp = await fetch(req.url, options)
            const respHeaders = {}

            resp.headers.forEach((value, key) => {
                respHeaders[key] = value
            })

            const contentType = resp.headers.get('Content-Type') || ''

            let body = null
            let bodyBase64 = null

            if (this.isTextContentType(contentType)) {
                try {
                    body = await resp.text()
                } catch {
                    const buffer = await resp.arrayBuffer()
                    bodyBase64 = await this.arrayBufferToBase64(buffer)
                }
            } else {
                const buffer = await resp.arrayBuffer()
                bodyBase64 = await this.arrayBufferToBase64(buffer)
            }

            return {
                id: req.id,
                type: 'fetch_response',
                status: resp.status,
                statusText: resp.statusText,
                body: body,
                bodyBase64: bodyBase64,
                redirected: resp.redirected,
                url: req.url,
            }
        } catch (err) {
            if (err.name === 'AbortError') {
                throw new Error(`[pipe] Fetch timed out after ${FETCH_TIMEOUT_MS} ms`)
            }
            throw err
        } finally {
            clearTimeout(timer)
        }
    }

    isTextContentType(contentType) {
        if (!contentType) return true;
        const ct = contentType.toLowerCase();
        return (
            ct.startsWith("text/") ||
            ct.includes("json") ||
            ct.includes("xml") ||
            ct.includes("javascript") ||
            ct.includes("css") ||
            ct.includes("html") ||
            ct.includes("svg") ||
            ct.includes("urlencoded")
        );
    }

    async arrayBufferToBase64(buffer) {
        const bytes = new Uint8Array(buffer);
        const chunkSize = 8192

        let binary = ''

        for (let i = 0; i < bytes.length; i += chunkSize) {
            const chunk = bytes.slice(i, i + chunkSize);
            binary += String.fromCharCode.apply(null, chunk)
        }

        return btoa(binary)
    }
}

(() => {
    const pipe = new BrowserPipe()
    pipe.start()
})()
