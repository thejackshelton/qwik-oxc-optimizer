
		import { wrapperFn, getEnv } from 'utils';
		import { formAction$, valiForm$ } from 'forms';

		const flagEnabled = getEnv().PUBLIC_FEATURE;
		export const FeatureSchema = wrapperFn(flagEnabled);

		export const featureAction = formAction$(async (requestEvent) => {
			return {
				status: 'success',
			};
		}, valiForm$(FeatureSchema));
		