
		import { component$ } from "@qwik.dev/core";
		import { globalAction$ } from "@qwik.dev/router";

		export const useSecretAction = globalAction$(
			async (payload) => console.log(payload) || 'hi'
		);

		export const SecretForm = component$(() => {
			const action = useSecretAction();
			return <div>{action.value}</div>
		});
		