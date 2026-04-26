const WS_URL = 'ws://127.0.0.1:10129'
const RECONNECT_BASE_MS = 1000
const RECONNECT_MAX_MS = 30000
const RECONNECT_JITTER_MS = 1000
const FETCH_TIMEOUT_MS = 30000
const KEEPALIVE_ALARM_NAME = 'keepalive'
const FRAME_READY_TIMEOUT_MS = 15000

class BrowserPipe {
    constructor() {
        this.ws = null
        this.reconnectAttempt = 0
        this.reconnectTimer = null

        // iframe fetch state
        this.pendingFrameReady = {}
        this.pendingResults = {}
        this.registeredHosts = new Set()

        chrome.runtime.onMessage.addListener((msg, sender, sendResponse) => {
            if (msg.type === 'frame_ready') {
                this.pendingFrameReady[msg.requestId] = sendResponse
                return true // keep channel open for async sendResponse
            } else if (msg.type === 'fetch_result' || msg.type === 'fetch_error') {
                this.pendingResults[msg.requestId] = msg
            }
        })
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
                const resp = msg.referrer
                    ? await this.fetchViaIframe(msg)
                    : await this.fetch(msg)
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

    // ── Direct fetch (existing behavior) ──

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

    // ── Iframe fetch (offscreen document + content script) ──

    async ensureOffscreen() {
        const contexts = await chrome.runtime.getContexts({
            contextTypes: ['OFFSCREEN_DOCUMENT']
        })
        if (contexts.length > 0) return

        await chrome.offscreen.createDocument({
            url: 'offscreen.html',
            reasons: ['IFRAME_SCRIPTING'],
            justification: 'Fetch URLs with correct cookie partition via iframe'
        })
    }

    async ensureContentScriptForHost(hostname) {
        if (this.registeredHosts.has(hostname)) return

        const scriptId = 'bpipe-' + hostname.replace(/[^a-zA-Z0-9]/g, '-')

        const existing = await chrome.scripting.getRegisteredContentScripts({ ids: [scriptId] })
        if (existing.length === 0) {
            await chrome.scripting.registerContentScripts([{
                id: scriptId,
                matches: ['https://' + hostname + '/*', 'http://' + hostname + '/*'],
                js: ['content.js'],
                runAt: 'document_idle',
                allFrames: true
            }])
        }

        this.registeredHosts.add(hostname)
    }

    async fetchViaIframe(req) {
        const cleanup = () => {
            chrome.runtime.sendMessage({ action: 'remove_iframe', id: req.id })
                .catch(() => {})
            delete this.pendingFrameReady[req.id]
            delete this.pendingResults[req.id]
        }

        try {
            // 1. Ensure offscreen document exists
            await this.ensureOffscreen()

            // 2. Register content script for the target host
            const hostname = new URL(req.referrer).hostname
            await this.ensureContentScriptForHost(hostname)

            // 3. Create iframe and wait for it to load
            const iframeReady = await chrome.runtime.sendMessage({
                action: 'create_iframe',
                id: req.id,
                url: req.referrer
            })
            if (!iframeReady?.ok) {
                throw new Error(iframeReady?.error || 'Failed to create iframe')
            }

            // 4. Wait for content script to signal ready
            const sendResponse = await this.pollPending(this.pendingFrameReady, req.id, FRAME_READY_TIMEOUT_MS)

            // 5. Send fetch parameters to content script
            sendResponse({
                url: req.url,
                method: req.method || 'GET',
                headers: req.headers || {},
                body: req.body || null,
                redirect: req.redirect || 'follow'
            })

            // 6. Wait for fetch result
            const result = await this.pollPending(this.pendingResults, req.id, FETCH_TIMEOUT_MS)

            // 7. Cleanup iframe
            cleanup()

            // 8. Build response
            if (result.type === 'fetch_error') {
                return {
                    id: req.id,
                    type: 'fetch_error',
                    error: result.error
                }
            }

            return {
                id: req.id,
                type: 'fetch_response',
                status: result.status,
                statusText: result.statusText || '',
                body: result.body || null,
                bodyBase64: result.bodyBase64 || null,
                redirected: result.redirected || false,
                url: result.url || req.url
            }
        } catch (err) {
            cleanup()
            throw err
        }
    }

    pollPending(map, key, timeout) {
        return new Promise((resolve, reject) => {
            const start = Date.now()
            const interval = setInterval(() => {
                if (map[key]) {
                    clearInterval(interval)
                    const value = map[key]
                    delete map[key]
                    resolve(value)
                }
                if (Date.now() - start > timeout) {
                    clearInterval(interval)
                    reject(new Error(`Timed out waiting for ${key}`))
                }
            }, 100)
        })
    }

    // ── Utilities ──

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
