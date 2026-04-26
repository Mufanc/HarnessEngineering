const FETCH_TIMEOUT_MS = 30000

;(async () => {
    const hash = location.hash
    if (!hash.startsWith('#bpipe-')) return

    const requestId = hash.slice(7)

    try {
        const params = await new Promise((resolve, reject) => {
            chrome.runtime.sendMessage(
                { type: 'frame_ready', requestId },
                (response) => {
                    if (chrome.runtime.lastError) {
                        reject(new Error(chrome.runtime.lastError.message))
                    } else {
                        resolve(response)
                    }
                }
            )
        })

        const headers = new Headers(params.headers || {})

        const controller = new AbortController()
        const timer = setTimeout(() => controller.abort(), FETCH_TIMEOUT_MS)

        try {
            const options = {
                method: params.method || 'GET',
                headers,
                credentials: 'include',
                signal: controller.signal,
                redirect: params.redirect || 'follow'
            }

            if (params.body && !['GET', 'HEAD'].includes(options.method.toUpperCase())) {
                options.body = params.body
            }

            const resp = await fetch(params.url, options)
            const contentType = resp.headers.get('Content-Type') || ''

            let body = null
            let bodyBase64 = null

            if (isTextContentType(contentType)) {
                try {
                    body = await resp.text()
                } catch {
                    const buffer = await resp.arrayBuffer()
                    bodyBase64 = arrayBufferToBase64(buffer)
                }
            } else {
                const buffer = await resp.arrayBuffer()
                bodyBase64 = arrayBufferToBase64(buffer)
            }

            chrome.runtime.sendMessage({
                type: 'fetch_result',
                requestId,
                status: resp.status,
                statusText: resp.statusText,
                body,
                bodyBase64,
                redirected: resp.redirected,
                url: resp.url
            })
        } catch (err) {
            if (err.name === 'AbortError') {
                throw new Error(`Fetch timed out after ${FETCH_TIMEOUT_MS} ms`)
            }
            throw err
        } finally {
            clearTimeout(timer)
        }
    } catch (err) {
        chrome.runtime.sendMessage({
            type: 'fetch_error',
            requestId,
            error: err.message || String(err)
        })
    }
})()

function isTextContentType(contentType) {
    if (!contentType) return true
    const ct = contentType.toLowerCase()
    return (
        ct.startsWith('text/') ||
        ct.includes('json') ||
        ct.includes('xml') ||
        ct.includes('javascript') ||
        ct.includes('css') ||
        ct.includes('html') ||
        ct.includes('svg') ||
        ct.includes('urlencoded')
    )
}

function arrayBufferToBase64(buffer) {
    const bytes = new Uint8Array(buffer)
    const chunkSize = 8192
    let binary = ''
    for (let i = 0; i < bytes.length; i += chunkSize) {
        const chunk = bytes.slice(i, i + chunkSize)
        binary += String.fromCharCode.apply(null, chunk)
    }
    return btoa(binary)
}
