import { gitlab } from '@/ipc/client'
import { EmptyState } from '@/kit/components/Feedback'
import { Code, Person } from '@/kit/components/Status'
import { List, ListItem } from '@/kit/components/Surface'
import { data, useGitLab } from './store'
import chrome from './ReviewChrome.module.css'

/**
 * The MR's commits, newest first, as GitLab lists them. A row opens the commit on GitLab: cide
 * has no checkout of an arbitrary MR commit to diff against, and the review's own diff (base →
 * head) is already the tab the reader came from.
 */
export function Commits({ review }: { review: string }) {
  useGitLab()
  const commits = data.get(review)?.commits
  if (commits === null)
    return (
      <EmptyState
        icon="circle-alert"
        title="GitLab did not list the commits"
      >
        Refresh the merge request to try again.
      </EmptyState>
    )
  if (!commits?.length)
    return <EmptyState icon="git-commit-horizontal" title="No commits" />
  return (
    <div className={chrome.commits}>
      <List label="Commits">
        {commits.map((commit) => (
          <ListItem
            key={commit.id}
            top={<Code>{commit.short_id}</Code>}
            topAside={
              <time dateTime={commit.authored_date} title={commit.authored_date}>
                {new Date(commit.authored_date).toLocaleString()}
              </time>
            }
            title={commit.title}
            meta={<Person name={commit.author_name} />}
            onOpen={
              commit.web_url.startsWith('https://') || commit.web_url.startsWith('http://')
                ? () => void gitlab.openUrl(commit.web_url)
                : undefined
            }
          />
        ))}
      </List>
    </div>
  )
}
