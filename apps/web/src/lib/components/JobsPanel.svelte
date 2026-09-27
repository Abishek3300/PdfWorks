<script lang="ts">
	// Feature: pdf-tools-suite (Task 11.3, Req 48.5)
	//
	// Shows each running Job's status independently. Rendered in the shell so a
	// User running several tools in one session sees every job's progress.

	import { jobs, removeJob, type JobRecord } from '$lib/stores/jobs';

	function phaseLabel(job: JobRecord): string {
		switch (job.phase) {
			case 'queued':
				return 'Queued';
			case 'running':
				return job.progress != null ? `Working ${job.progress}%` : 'Working…';
			case 'succeeded':
				return 'Done';
			case 'failed':
				return 'Failed';
		}
	}
</script>

{#if $jobs.length > 0}
	<section class="jobs card" aria-label="Active jobs">
		<h2>Jobs this session</h2>
		<ul>
			{#each $jobs as job (job.id)}
				<li class="job job--{job.phase}">
					<div class="job-main">
						<span class="job-tool">{job.toolLabel}</span>
						<span class="badge badge--{job.mode === 'Client_Side' ? 'device' : 'server'}">
							{job.mode === 'Client_Side' ? 'On device' : 'Server'}
						</span>
					</div>
					<div class="job-status">
						<span class="job-phase">{phaseLabel(job)}</span>
						{#if job.phase === 'running' && job.progress != null}
							<span
								class="bar"
								role="progressbar"
								aria-valuemin="0"
								aria-valuemax="100"
								aria-valuenow={job.progress}
							>
								<span class="bar-fill" style="width:{job.progress}%"></span>
							</span>
						{/if}
						{#if job.error}
							<span class="job-error">{job.error}</span>
						{/if}
					</div>
					{#if job.phase === 'succeeded' || job.phase === 'failed'}
						<button class="btn btn--ghost dismiss" on:click={() => removeJob(job.id)}>
							Dismiss<span class="sr-only"> {job.toolLabel} job</span>
						</button>
					{/if}
				</li>
			{/each}
		</ul>
	</section>
{/if}

<style>
	.jobs {
		padding: 1rem 1.1rem;
		margin-top: 1.5rem;
	}
	.jobs h2 {
		font-size: 1rem;
		margin-bottom: 0.75rem;
	}
	ul {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: 0.6rem;
	}
	.job {
		display: grid;
		grid-template-columns: 1fr auto;
		gap: 0.35rem 0.75rem;
		align-items: center;
		padding: 0.6rem 0.75rem;
		border: 1px solid var(--border);
		border-radius: var(--radius-sm);
		background: var(--surface-2);
	}
	.job-main {
		display: flex;
		align-items: center;
		gap: 0.5rem;
	}
	.job-tool {
		font-weight: 600;
	}
	.job-status {
		grid-column: 1 / -1;
		display: flex;
		align-items: center;
		gap: 0.6rem;
		font-size: 0.88rem;
		color: var(--ink-soft);
	}
	.job--failed .job-status {
		color: var(--danger);
	}
	.bar {
		flex: 1;
		height: 8px;
		border-radius: 999px;
		background: var(--border);
		overflow: hidden;
		max-width: 12rem;
	}
	.bar-fill {
		display: block;
		height: 100%;
		background: var(--accent);
	}
	.dismiss {
		min-height: 36px;
		padding: 0.3rem 0.7rem;
		font-size: 0.85rem;
	}
</style>
