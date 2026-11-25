type Listener = (value: unknown) => void;

const pending = new Map<number, { resolve: Listener; reject: Listener }>();
const listeners = new Map<string, Set<Listener>>();
let sequence = 0;

const host = window as unknown as Window & {
  webkit: { messageHandlers: { guanhai: { postMessage: (message: string) => void } } };
  guanhaiReply: (id: number, value: unknown, error: boolean) => void;
  guanhaiEvent: (name: string, payload: unknown) => void;
};

host.guanhaiReply = (id, value, error) => {
  const callback = pending.get(id);
  pending.delete(id);
  if (callback) (error ? callback.reject : callback.resolve)(value);
};
host.guanhaiEvent = (name, payload) => {
  listeners.get(name)?.forEach(listener => listener(payload));
};

export function invoke<T>(command: string, args?: unknown): Promise<T> {
  return new Promise((resolve, reject) => {
    const id = ++sequence;
    pending.set(id, { resolve: value => resolve(value as T), reject });
    try {
      host.webkit.messageHandlers.guanhai.postMessage(JSON.stringify({ id, command, args }));
    } catch (error) {
      pending.delete(id);
      reject(error);
    }
  });
}

export async function listen<T>(name: string, callback: (event: { payload: T }) => void) {
  const group = listeners.get(name) ?? new Set<Listener>();
  listeners.set(name, group);
  const listener: Listener = payload => callback({ payload: payload as T });
  group.add(listener);
  return () => { group.delete(listener); };
}

export function getCurrentWindow() {
  return {
    onDragDropEvent: (callback: (event: { payload: { type: string; paths: string[] } }) => void) =>
      listen("drag-drop", callback),
  };
}

export function open(options: { multiple: boolean; directory: boolean; filters: { name: string; extensions: string[] }[] }) {
  return invoke<string[] | null>("open", options);
}

export function save(options: { defaultPath: string }) {
  return invoke<string | null>("save", options);
}
