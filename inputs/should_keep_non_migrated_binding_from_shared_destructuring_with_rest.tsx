
import { $ } from '@qwik.dev/core';

const { a, ...rest } = { a: 1, b: 2, c: 3 };

console.log('root', rest.b, rest.c);

export const handler = $(() => {
  console.log('qrl', a);
});

		