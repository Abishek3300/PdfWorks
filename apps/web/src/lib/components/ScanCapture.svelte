<script lang="ts">
	// Feature: pdf-tools-suite (Task 10.3, Req 9.1, 9.4)
	//
	// Camera capture for Scan to PDF, with an upload fallback when the camera is
	// unavailable or access is denied (Req 9.4). Captured frames are emitted as
	// image Files via a `capture` event so the workspace treats them exactly like
	// uploaded images.

	import { createEventDispatcher, onDestroy } from 'svelte';

	const dispatch = createEventDispatcher<{ capture: { file: File } }>();

	let stream: MediaStream | null = null;
	let video: HTMLVideoElement;
	let active = false;
	let error: string | null = null;
	let frame = 0;

	async function startCamera() {
		error = null;
		if (!navigator.mediaDevices?.getUserMedia) {
			error = 'This browser has no camera access. You can upload images instead.';
			return;
		}
		try {
			stream = await navigator.mediaDevices.getUserMedia({ video: { facingMode: 'environment' } });
			active = true;
			// Assign after the element renders.
			queueMicrotask(() => {
				if (video && stream) {
					video.srcObject = stream;
					void video.play();
				}
			});
		} catch {
			// Access denied or unavailable — fall back to upload (Req 9.4).
			error = 'Camera access was denied. You can upload images instead.';
			active = false;
		}
	}

	function stopCamera() {
		stream?.getTracks().forEach((t) => t.stop());
		stream = null;
		active = false;
	}

	function capture() {
		if (!video) return;
		const canvas = document.createElement('canvas');
		canvas.width = video.videoWidth || 1280;
		canvas.height = video.videoHeight || 720;
		const ctx = canvas.getContext('2d');
		if (!ctx) return;
		ctx.drawImage(video, 0, 0, canvas.width, canvas.height);
		canvas.toBlob((blob) => {
			if (!blob) return;
			frame += 1;
			const file = new File([blob], `scan-${frame}.jpg`, { type: 'image/jpeg' });
			dispatch('capture', { file });
		}, 'image/jpeg', 0.92);
	}

	onDestroy(stopCamera);
</script>

<div class="scan">
	{#if !active}
		<button class="btn btn--primary" on:click={startCamera}>Use camera</button>
	{:else}
		<!-- eslint-disable-next-line -->
		<video bind:this={video} class="preview" muted playsinline aria-label="Camera preview"></video>
		<div class="scan-actions">
			<button class="btn btn--primary" on:click={capture}>Capture</button>
			<button class="btn" on:click={stopCamera}>Stop camera</button>
		</div>
	{/if}

	{#if error}
		<p class="notice notice--warn" role="status">{error}</p>
	{/if}
</div>

<style>
	.scan {
		display: flex;
		flex-direction: column;
		gap: 0.75rem;
		margin-bottom: 1rem;
	}
	.preview {
		width: 100%;
		max-height: 18rem;
		background: #000;
		border-radius: var(--radius-sm);
	}
	.scan-actions {
		display: flex;
		gap: 0.5rem;
	}
</style>
