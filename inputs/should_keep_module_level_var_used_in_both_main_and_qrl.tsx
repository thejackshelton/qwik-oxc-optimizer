
import { $, isServer } from '@qwik.dev/core';

const state = {
  counter: 0,
  id: Math.random().toString(36).slice(2, 8),
};

console.log('Module init, id:', state.id);

export function doSomething() {
  if (!isServer) {
    state.counter++;
  }
}

export function useHook() {
  const fn = $(() => {
    state.counter++;
    console.log(state.id);
  });
  return fn;
}

		