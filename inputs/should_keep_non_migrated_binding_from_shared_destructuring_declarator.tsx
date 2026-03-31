
import { $ } from '@qwik.dev/core';

const { a, b } = {
  a: 1,
  b: 2,
};

console.log('root', b);

export const handler = $(() => {
  console.log('qrl', a);
});

		