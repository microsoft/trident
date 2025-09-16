import clsx from 'clsx';
import Heading from '@theme/Heading';
import styles from './styles.module.css';

const FeatureList = [
  {
    title: 'Install',
    // Svg: require('@site/static/img/trident_install.svg').default,
    description: (
      <>
        Trident is great for installing Azure Linux.
      </>
    ),
  },
  {
    title: 'Service ',
    // Svg: require('@site/static/img/trident_service.svg').default,
    description: (
      <>
        Trident helps you manage and update Azure Linux.
      </>
    ),
  },
];

function Feature({ title, description }) {
  return (
    <div className={clsx('col col--4')}>
      <div className="text--center padding-horiz--md">
        <Heading as="h3">{title}</Heading>
        <p>{description}</p>
      </div>
    </div>
  );
}

export default function HomepageFeatures() {
  return (
    <section className={styles.features}>
      <div className="container">
        <div className="row">
          {FeatureList.map((props, idx) => (
            <Feature key={idx} {...props} />
          ))}
        </div>
      </div>
    </section>
  );
}
