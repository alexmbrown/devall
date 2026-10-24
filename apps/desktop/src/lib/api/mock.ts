import { invoke } from '@tauri-apps/api/core';

/** A mapping file under `mocks/mappings/` that could not be loaded (path relative to the project root). */
export interface MockLoadError {
  file: string;
  error: string;
}

/** Mirrors `MockStatus` in src-tauri/src/modules/mock.rs. */
export interface MockStatus {
  root: string;
  running: boolean;
  port: number | null;
  /** `http://127.0.0.1:<port>`; the WireMock-compatible admin API is at `${url}/__admin`. */
  url: string | null;
  stubCount: number;
  loadErrors: MockLoadError[];
  /** gRPC / MQTT listeners, GraphQL endpoints, WebSocket stubs (null while stopped). */
  protocols: ProtocolsInfo | null;
}

/** Mirrors devall_mock::ProtocolsInfo. */
export interface ProtocolsInfo {
  /** GraphQL endpoint paths on the HTTP port. */
  graphql: string[];
  websocketStubs: number;
  websocketConnections: number;
  grpcPort: number | null;
  grpcMethods: number;
  mqttPort: number | null;
  mqttClients: number;
}

/** A file under mocks/graphql|websocket|grpc|mqtt (mirrors ProtocolFile in modules/mock.rs). */
export interface ProtocolFile {
  protocol: 'graphql' | 'websocket' | 'grpc' | 'mqtt';
  rel: string;
  path: string;
  size: number;
}

export interface ProtocolFolders {
  graphql: boolean;
  websocket: boolean;
  grpc: boolean;
  mqtt: boolean;
}

/** A CSV / JSON / SQLite file under `mocks/data/` (mirrors devall_mock::DataSourceInfo). */
export interface DataSourceInfo {
  /** Name for helpers: `users`, `eu/products`, `shop.orders`. */
  name: string;
  kind: 'csv' | 'json' | 'sqlite';
  /** Relative to the project root. */
  file: string;
  columns: string[];
  rows: number;
  error?: string;
}

/** Typed wrappers over the mock server commands. Errors reject with a string. */
export const mockApi = {
  /** Starts (or returns) the project's mock server. `port` omitted or 0 = any free port. */
  start: (root: string, port?: number, bindAll?: boolean) => invoke<MockStatus>('mock_start', { root, port, bindAll }),
  stop: (root: string) => invoke<void>('mock_stop', { root }),
  status: (root: string) => invoke<MockStatus>('mock_status', { root }),
  list: () => invoke<MockStatus[]>('mock_list'),
  /** Data sources in `mocks/data/` (works without a running server). */
  dataSources: (root: string) => invoke<DataSourceInfo[]>('mock_data_sources', { root }),
  /** Starts / moves the gRPC listener (`port` 0 or omitted = any free port). */
  grpcStart: (root: string, port?: number) => invoke<MockStatus>('mock_grpc_start', { root, port }),
  grpcStop: (root: string) => invoke<MockStatus>('mock_grpc_stop', { root }),
  mqttStart: (root: string, port?: number) => invoke<MockStatus>('mock_mqtt_start', { root, port }),
  mqttStop: (root: string) => invoke<MockStatus>('mock_mqtt_stop', { root }),
  /** Files under mocks/graphql|websocket|grpc|mqtt (works without a running server). */
  protocolFiles: (root: string) => invoke<[ProtocolFolders, ProtocolFile[]]>('mock_protocol_files', { root }),
  /** Writes a protocol file; returns its absolute path. */
  protocolWrite: (root: string, rel: string, content: string) => invoke<string>('mock_protocol_write', { root, rel, content }),
  /** Moves a protocol file to the OS trash. */
  protocolDelete: (root: string, rel: string) => invoke<void>('mock_protocol_delete', { root, rel }),
};

/** Base URL of a running server's admin API, for pointing the WireMock UI client at it. */
export function mockAdminUrl(status: MockStatus): string | null {
  return status.url ? `${status.url}/__admin` : null;
}
