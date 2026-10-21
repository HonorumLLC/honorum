import Foundation

/// The Honorum HTTP API. Every response is JSON, every catalog publication is
/// immutable and content addressed, so the URL cache can hold them without a
/// revalidation round trip.
struct HonorumAPI: Sendable {
    var origin = URL(string: "https://honorum.app")!

    private static let session: URLSession = {
        let configuration = URLSessionConfiguration.default
        configuration.waitsForConnectivity = true
        configuration.timeoutIntervalForRequest = 30
        configuration.httpMaximumConnectionsPerHost = 4
        configuration.urlCache = URLCache(
            memoryCapacity: 8 << 20, diskCapacity: 128 << 20, directory: nil)
        return URLSession(configuration: configuration)
    }()

    private static let decoder: JSONDecoder = {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return decoder
    }()

    /// A request the server refused, carrying the sentence it sent back so the
    /// interface can show the server's own wording rather than inventing one.
    enum Failure: LocalizedError {
        case status(Int, String)
        case notFound
        case unauthorized
        case rateLimited(String, retryAfter: Int)
        case offline

        var errorDescription: String? {
            switch self {
            case .notFound: "That was not found on the server."
            case .unauthorized: "This device is signed out. Sign in again to sync."
            case .offline: "Honorum could not reach the server."
            case .rateLimited(let message, _): message
            case .status(let code, let message):
                message.isEmpty ? "The server replied with \(code)." : message
            }
        }
    }

    // MARK: Public data

    func configuration() async throws -> AppConfig {
        try await get(origin.appending(path: "api/config"), immutable: false)
    }

    /// A published structure file, named by its content hash.
    func structure(inst: String, term: String, hash: String) async throws -> CatalogFile {
        try await get(catalogURL(inst, term, kind: "structure", hash: hash), immutable: true)
    }

    func seats(inst: String, term: String, hash: String) async throws -> SeatsFile {
        try await get(catalogURL(inst, term, kind: "seats", hash: hash), immutable: true)
    }

    /// Current counts for up to 100 sections, for classes on screen or watched.
    func liveSeats(inst: String, term: String, ids: [String]) async throws -> [String: Seats] {
        guard !ids.isEmpty else { return [:] }
        var components = URLComponents(
            url: origin.appending(path: "api/seats"), resolvingAgainstBaseURL: false)!
        components.queryItems = [
            URLQueryItem(name: "inst", value: inst),
            URLQueryItem(name: "term", value: term),
            URLQueryItem(name: "ids", value: ids.prefix(100).joined(separator: ",")),
        ]
        struct Row: Codable { var id: String; var available: Int; var capacity: Int; var waitlist: Int }
        let rows: [Row] = try await get(components.url!, immutable: false)
        return rows.reduce(into: [:]) {
            $0[$1.id] = Seats(capacity: $1.capacity, available: $1.available, waitlist: $1.waitlist)
        }
    }

    // MARK: Transport

    private func catalogURL(_ inst: String, _ term: String, kind: String, hash: String) -> URL {
        origin.appending(path: "catalog").appending(path: inst).appending(path: "\(term).\(kind).\(hash).json")
    }

    private func get<T: Decodable>(_ url: URL, immutable: Bool) async throws -> T {
        var request = URLRequest(url: url)
        request.cachePolicy = immutable ? .returnCacheDataElseLoad : .useProtocolCachePolicy
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        return try Self.decoder.decode(T.self, from: try await perform(request))
    }

    /// A JSON request that expects a JSON reply.
    func send<T: Decodable>(
        _ method: String, _ path: String, body: [String: JSON]? = nil, token: String? = nil,
        timeout: TimeInterval = 30
    ) async throws -> T {
        let data = try await raw(method, path, body: body, token: token, timeout: timeout)
        return try Self.decoder.decode(T.self, from: data)
    }

    /// A JSON request whose reply carries nothing this client needs.
    func sendVoid(
        _ method: String, _ path: String, body: [String: JSON]? = nil, token: String? = nil,
        timeout: TimeInterval = 30
    ) async throws {
        _ = try await raw(method, path, body: body, token: token, timeout: timeout)
    }

    func raw(
        _ method: String, _ path: String, body: [String: JSON]? = nil, token: String? = nil,
        timeout: TimeInterval = 30
    ) async throws -> Data {
        var request = URLRequest(url: origin.appending(path: path))
        request.httpMethod = method
        request.timeoutInterval = timeout
        request.cachePolicy = .reloadIgnoringLocalCacheData
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        if let body {
            request.httpBody = JSON.object(body).data
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        }
        if let token, !token.isEmpty {
            request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        return try await perform(request)
    }

    /// A long-lived response body: the account's live update stream. Server
    /// heartbeats keep it inside the request timeout while nothing changes.
    func stream(_ path: String, query: [String: String] = [:], token: String) async throws -> URLSession.AsyncBytes {
        var components = URLComponents(url: origin.appending(path: path), resolvingAgainstBaseURL: false)!
        if !query.isEmpty { components.queryItems = query.map { URLQueryItem(name: $0.key, value: $0.value) } }
        var request = URLRequest(url: components.url!)
        request.timeoutInterval = 90
        request.cachePolicy = .reloadIgnoringLocalCacheData
        request.setValue("text/event-stream", forHTTPHeaderField: "Accept")
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        let (bytes, response): (URLSession.AsyncBytes, URLResponse)
        do {
            (bytes, response) = try await Self.session.bytes(for: request)
        } catch let error as URLError {
            throw error.code == .cancelled ? error : Failure.offline
        }
        if let http = response as? HTTPURLResponse, !(200..<300).contains(http.statusCode) {
            switch http.statusCode {
            case 401, 403: throw Failure.unauthorized
            default: throw Failure.status(http.statusCode, "")
            }
        }
        return bytes
    }

    private func perform(_ request: URLRequest) async throws -> Data {
        let data: Data
        let response: URLResponse
        do {
            (data, response) = try await Self.session.data(for: request)
        } catch let error as URLError {
            throw error.code == .cancelled ? error : Failure.offline
        }
        guard let http = response as? HTTPURLResponse else { return data }
        guard !(200..<300).contains(http.statusCode) else { return data }
        let message = JSON.decode(data)?.text("error") ?? ""
        switch http.statusCode {
        case 401, 403: throw Failure.unauthorized
        case 404: throw Failure.notFound
        case 429:
            let after = Int(http.value(forHTTPHeaderField: "Retry-After") ?? "") ?? 30
            throw Failure.rateLimited(message, retryAfter: after)
        default: throw Failure.status(http.statusCode, message)
        }
    }
}
